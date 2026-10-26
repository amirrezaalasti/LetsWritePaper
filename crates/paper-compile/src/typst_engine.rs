//! In-process Typst compilation over live (in-memory) sources.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use typst::diag::{FileResult, Severity, SourceDiagnostic};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{DiagSpanKind, FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_kit::files::{FileLoader, FileStore, FsRoot, SystemFiles};
use typst_kit::fonts::FontStore;
use typst_kit::packages::SystemPackages;
use typst_layout::PagedDocument;

use crate::{CompileOptions, CompileOutput, Diagnostic};

/// Downloads Typst Universe packages over rustls (no OpenSSL dependency).
struct UreqDownloader;

impl typst_kit::downloader::Downloader for UreqDownloader {
    fn stream(&self, _key: &dyn std::any::Any, url: &str) -> std::io::Result<(Option<usize>, Box<dyn Read>)> {
        let response = ureq::get(url)
            .header("User-Agent", concat!("letswritepaper/", env!("CARGO_PKG_VERSION")))
            .call()
            .map_err(std::io::Error::other)?;
        let len = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        Ok((len, Box::new(response.into_body().into_reader())))
    }
}

/// Serves live text for project files, falling back to disk and packages.
struct OverlayLoader {
    overlay: HashMap<String, String>,
    system: Arc<SystemFiles>,
}

impl FileLoader for OverlayLoader {
    fn load(&self, id: FileId) -> FileResult<Bytes> {
        if let VirtualRoot::Project = id.root() {
            let key = id.vpath().get_without_slash().to_string();
            if let Some(text) = self.overlay.get(&key) {
                return Ok(Bytes::from_string(text.clone()));
            }
        }
        self.system.load(id)
    }
}

pub struct TypstEngine {
    library: LazyHash<Library>,
    fonts: FontStore,
}

struct PaperWorld<'a> {
    engine: &'a TypstEngine,
    files: FileStore<OverlayLoader>,
    main: FileId,
}

impl World for PaperWorld<'_> {
    fn library(&self) -> &LazyHash<Library> {
        &self.engine.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        self.engine.fonts.book()
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        self.files.source(id)
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.files.file(id)
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.engine.fonts.font(index)
    }

    fn today(&self, offset: Option<Duration>) -> Option<Datetime> {
        let mut now = time::OffsetDateTime::now_utc();
        if let Some(o) = offset {
            now += time::Duration::hours(o.hours() as i64);
        }
        Datetime::from_ymd(now.year(), now.month() as u8, now.day())
    }
}

fn file_id(path: &str) -> Option<FileId> {
    let vpath = VirtualPath::new(path).ok()?;
    Some(RootedPath::new(VirtualRoot::Project, vpath).intern())
}

impl TypstEngine {
    pub fn new() -> Self {
        let mut fonts = FontStore::new();
        fonts.extend(typst_kit::fonts::embedded());
        fonts.extend(typst_kit::fonts::system());
        TypstEngine { library: LazyHash::new(Library::default()), fonts }
    }

    pub fn compile(&self, root: &Path, main: &str, sources: &HashMap<String, String>, opts: CompileOptions) -> CompileOutput {
        let mut out = CompileOutput { engine: "typst".into(), ..Default::default() };
        let Some(main_id) = file_id(main) else {
            out.diagnostics.push(error(format!("invalid main file path: {main}")));
            return out;
        };
        let system = SystemFiles::new(FsRoot::new(root.to_path_buf()), SystemPackages::new(UreqDownloader));
        let world = PaperWorld {
            engine: self,
            files: FileStore::new(OverlayLoader { overlay: sources.clone(), system: Arc::new(system) }),
            main: main_id,
        };
        let result = typst::compile::<PagedDocument>(&world);
        out.diagnostics.extend(result.warnings.iter().map(|d| convert(&world, d)));
        match result.output {
            Ok(doc) => {
                out.page_count = doc.pages().len();
                if opts.svg {
                    let svg_opts = typst_svg::SvgOptions::default();
                    out.svg_pages = doc.pages().iter().map(|p| typst_svg::svg(p, &svg_opts)).collect();
                }
                if opts.pdf {
                    match typst_pdf::pdf(&doc, &typst_pdf::PdfOptions::default()) {
                        Ok(bytes) => out.pdf = Some(bytes),
                        Err(errs) => out.diagnostics.extend(errs.iter().map(|d| convert(&world, d))),
                    }
                }
                out.ok = !out.diagnostics.iter().any(|d| d.severity == "error");
            }
            Err(errs) => out.diagnostics.extend(errs.iter().map(|d| convert(&world, d))),
        }
        out
    }
}

fn error(message: String) -> Diagnostic {
    Diagnostic { severity: "error".into(), file: None, line: None, message, hints: vec![] }
}

fn convert(world: &PaperWorld, d: &SourceDiagnostic) -> Diagnostic {
    let (file, line) = locate(world, d);
    Diagnostic {
        severity: match d.severity {
            Severity::Error => "error".into(),
            Severity::Warning => "warning".into(),
        },
        file,
        line,
        message: d.message.to_string(),
        hints: d.hints.iter().map(|h| h.v.to_string()).collect(),
    }
}

fn locate(world: &PaperWorld, d: &SourceDiagnostic) -> (Option<String>, Option<usize>) {
    use typst::WorldExt;
    let id = match d.span.get() {
        DiagSpanKind::Detached => return (None, None),
        DiagSpanKind::Number { id, .. } | DiagSpanKind::Range { id, .. } => id,
    };
    let name = match id.root() {
        VirtualRoot::Project => id.vpath().get_without_slash().to_string(),
        VirtualRoot::Package(spec) => format!("{spec}/{}", id.vpath().get_without_slash()),
    };
    let line = world.range(d.span).and_then(|r| {
        let src = world.source(id).ok()?;
        src.lines().byte_to_line(r.start).map(|l| l + 1)
    });
    (Some(name), line)
}
