//! Compilation for live preview and for agents checking their edits.
//!
//! Typst compiles in-process from in-memory sources (the live CRDT text), so
//! the preview never waits for a disk round-trip. LaTeX runs `tectonic` when
//! it is installed (including the copy the setup wizard places in the app's
//! bin directory), otherwise `latexmk`, on the files the owner process mirrors
//! to disk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use serde::{Deserialize, Serialize};

#[cfg(feature = "typst")]
mod typst_engine;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    /// `error` or `warning`.
    pub severity: String,
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<usize>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hints: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CompileOutput {
    pub ok: bool,
    pub engine: String,
    pub millis: u128,
    pub diagnostics: Vec<Diagnostic>,
    /// Where the PDF was written (LaTeX, or Typst when requested).
    pub pdf_path: Option<PathBuf>,
    /// PDF bytes (Typst only; LaTeX output is read from `pdf_path`).
    #[serde(skip)]
    pub pdf: Option<Vec<u8>>,
    /// One SVG per page (Typst only), for instant preview.
    #[serde(skip)]
    pub svg_pages: Vec<String>,
    pub page_count: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CompileOptions {
    pub svg: bool,
    pub pdf: bool,
}

/// Long-lived compiler; caches fonts and the Typst standard library.
#[derive(Default)]
pub struct Compiler {
    #[cfg(feature = "typst")]
    typst: Option<typst_engine::TypstEngine>,
}

impl Compiler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Compile `main` (project-relative). `sources` overrides file contents
    /// with live text (relative path → text); other files come from disk.
    pub fn compile(
        &mut self,
        root: &Path,
        main: &str,
        sources: &HashMap<String, String>,
        opts: CompileOptions,
    ) -> CompileOutput {
        let start = Instant::now();
        let mut out = if main.ends_with(".typ") {
            self.compile_typst(root, main, sources, opts)
        } else {
            compile_latex(root, main)
        };
        out.millis = start.elapsed().as_millis();
        out
    }

    #[cfg(feature = "typst")]
    fn compile_typst(
        &mut self,
        root: &Path,
        main: &str,
        sources: &HashMap<String, String>,
        opts: CompileOptions,
    ) -> CompileOutput {
        let engine = self
            .typst
            .get_or_insert_with(typst_engine::TypstEngine::new);
        let mut out = engine.compile(root, main, sources, opts);
        if let (Some(pdf), true) = (&out.pdf, opts.pdf) {
            let path = build_dir(root).join(
                Path::new(main)
                    .with_extension("pdf")
                    .file_name()
                    .unwrap_or_default(),
            );
            if std::fs::write(&path, pdf).is_ok() {
                out.pdf_path = Some(path);
            }
        }
        out
    }

    #[cfg(not(feature = "typst"))]
    fn compile_typst(
        &mut self,
        root: &Path,
        main: &str,
        _: &HashMap<String, String>,
        _: CompileOptions,
    ) -> CompileOutput {
        // Fall back to the typst CLI when built without the embedded compiler.
        let out_pdf = build_dir(root).join(
            Path::new(main)
                .with_extension("pdf")
                .file_name()
                .unwrap_or_default(),
        );
        let Some(tool) = find_tool("typst") else {
            return CompileOutput {
                ok: false,
                engine: "none".into(),
                diagnostics: vec![Diagnostic {
                    severity: "error".into(),
                    file: None,
                    line: None,
                    message:
                        "This build does not include Typst, and the typst program is not installed."
                            .into(),
                    hints: vec![],
                }],
                ..Default::default()
            };
        };
        run_tool(
            root,
            &tool,
            "typst",
            &["compile".as_ref(), main.as_ref(), out_pdf.as_os_str()],
            out_pdf,
            parse_typst_cli,
        )
    }
}

fn build_dir(root: &Path) -> PathBuf {
    let dir = root.join(".paper").join("build");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// The desktop app embeds Typst. Builds without that feature fall back to the CLI.
pub fn typst_embedded() -> bool {
    cfg!(feature = "typst")
}

fn exe_names(name: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![
            format!("{name}.exe"),
            format!("{name}.cmd"),
            format!("{name}.bat"),
            name.to_string(),
        ]
    } else {
        vec![name.to_string()]
    }
}

/// Directories searched for `tectonic`, `latexmk`, and `typst`.
/// The setup wizard's bin directory comes first, then the user and system paths.
fn tool_search_dirs(
    app_bin: PathBuf,
    home: Option<PathBuf>,
    path: impl IntoIterator<Item = PathBuf>,
) -> Vec<PathBuf> {
    let mut dirs = vec![app_bin];
    if let Some(home) = home {
        dirs.push(home.join(".local").join("bin"));
    }
    if cfg!(target_os = "macos") {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
    }
    for dir in path {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

fn path_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(registry_path_dirs().iter().cloned());
    dirs
}

fn registry_path_dirs() -> &'static [PathBuf] {
    static DIRS: std::sync::OnceLock<Vec<PathBuf>> = std::sync::OnceLock::new();
    DIRS.get_or_init(windows_registry_paths)
}

#[cfg(windows)]
fn windows_registry_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for scope in ["User", "Machine"] {
        let script = format!("[Environment]::GetEnvironmentVariable('Path','{scope}')");
        let Ok(output) = Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        out.extend(std::env::split_paths(text.trim()));
    }
    out
}

#[cfg(not(windows))]
fn windows_registry_paths() -> Vec<PathBuf> {
    Vec::new()
}

/// Full path of an installed program, if one is visible to this process.
pub fn find_tool(name: &str) -> Option<PathBuf> {
    let home =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    for dir in tool_search_dirs(paper_core::user_config::bin_dir(), home, path_dirs()) {
        for file in exe_names(name) {
            let candidate = dir.join(&file);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Which LaTeX engine would be used, if any.
pub fn latex_engine() -> Option<&'static str> {
    if find_tool("tectonic").is_some() {
        Some("tectonic")
    } else if find_tool("latexmk").is_some() {
        Some("latexmk")
    } else {
        None
    }
}

fn compile_latex(root: &Path, main: &str) -> CompileOutput {
    let out_dir = build_dir(root);
    let pdf = out_dir.join(
        Path::new(main)
            .with_extension("pdf")
            .file_name()
            .unwrap_or_default(),
    );
    match latex_engine() {
        Some(engine) => {
            let Some(tool) = find_tool(engine) else {
                return missing_latex();
            };
            if engine == "tectonic" {
                run_tool(
                    root,
                    &tool,
                    engine,
                    &[
                        "-X".as_ref(),
                        "compile".as_ref(),
                        main.as_ref(),
                        "--keep-logs".as_ref(),
                        "--outdir".as_ref(),
                        out_dir.as_os_str(),
                    ],
                    pdf,
                    parse_latex_log,
                )
            } else {
                let outdir = format!("-outdir={}", out_dir.display());
                run_tool(
                    root,
                    &tool,
                    engine,
                    &[
                        "-pdf".as_ref(),
                        "-interaction=nonstopmode".as_ref(),
                        "-file-line-error".as_ref(),
                        "-halt-on-error".as_ref(),
                        outdir.as_ref(),
                        main.as_ref(),
                    ],
                    pdf,
                    parse_latex_log,
                )
            }
        }
        None => missing_latex(),
    }
}

fn missing_latex() -> CompileOutput {
    CompileOutput {
        ok: false,
        engine: "none".into(),
        diagnostics: vec![Diagnostic {
            severity: "error".into(),
            file: None,
            line: None,
            message: "No LaTeX engine found. Open Set up this computer in LetsWritePaper to install tectonic, or install a TeX distribution that provides latexmk.".into(),
            hints: vec![],
        }],
        ..Default::default()
    }
}

fn run_tool(
    root: &Path,
    tool: &Path,
    engine: &str,
    args: &[&std::ffi::OsStr],
    pdf: PathBuf,
    parse: fn(&str, &Path) -> Vec<Diagnostic>,
) -> CompileOutput {
    let output = Command::new(tool).args(args).current_dir(root).output();
    let (status_ok, log) = match output {
        Ok(o) => (
            o.status.success(),
            format!(
                "{}\n{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            ),
        ),
        Err(e) => (false, format!("failed to run {engine}: {e}")),
    };
    let mut diagnostics = parse(&log, root);
    // latexmk/tectonic keep a .log with more detail than stdout.
    if let Some(stem) = pdf.file_stem() {
        if let Ok(full) =
            std::fs::read_to_string(pdf.with_file_name(format!("{}.log", stem.to_string_lossy())))
        {
            for d in parse(&full, root) {
                if !diagnostics
                    .iter()
                    .any(|x| x.message == d.message && x.line == d.line)
                {
                    diagnostics.push(d);
                }
            }
        }
    }
    if !status_ok && !diagnostics.iter().any(|d| d.severity == "error") {
        let tail: Vec<&str> = log.lines().rev().take(15).collect();
        diagnostics.push(Diagnostic {
            severity: "error".into(),
            file: None,
            line: None,
            message: tail.into_iter().rev().collect::<Vec<_>>().join("\n"),
            hints: vec![],
        });
    }
    CompileOutput {
        ok: status_ok,
        engine: engine.into(),
        pdf_path: pdf.exists().then_some(pdf),
        diagnostics,
        ..Default::default()
    }
}

/// Parse `file:line: message` errors (`-file-line-error`), `! message` /
/// `l.<n>` pairs, and LaTeX/BibTeX warnings.
pub fn parse_latex_log(log: &str, _root: &Path) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = vec![];
    let lines: Vec<&str> = log.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if let Some(d) = file_line_error(line) {
            out.push(d);
        } else if let Some(msg) = line.strip_prefix("! ") {
            let line_no = lines[i + 1..].iter().take(10).find_map(|l| {
                l.strip_prefix("l.")
                    .and_then(|r| r.split_whitespace().next())
                    .and_then(|n| n.parse().ok())
            });
            out.push(Diagnostic {
                severity: "error".into(),
                file: None,
                line: line_no,
                message: msg.trim().into(),
                hints: vec![],
            });
        } else if let Some(idx) = line.find("Warning: ") {
            if line.starts_with("LaTeX")
                || line.starts_with("Package")
                || line.contains("Citation")
                || line.contains("Reference")
            {
                let msg = line[idx + 9..].trim().to_string();
                let line_no = msg
                    .rsplit_once("on input line ")
                    .and_then(|(_, n)| n.trim_end_matches('.').parse().ok());
                out.push(Diagnostic {
                    severity: "warning".into(),
                    file: None,
                    line: line_no,
                    message: msg,
                    hints: vec![],
                });
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|d| seen.insert((d.message.clone(), d.line, d.file.clone())));
    out
}

fn file_line_error(line: &str) -> Option<Diagnostic> {
    // ./main.tex:12: Undefined control sequence.
    let mut parts = line.splitn(3, ':');
    let file = parts.next()?;
    let num: usize = parts.next()?.trim().parse().ok()?;
    let msg = parts.next()?.trim();
    if !(file.ends_with(".tex") || file.ends_with(".sty") || file.ends_with(".cls")) {
        return None;
    }
    Some(Diagnostic {
        severity: "error".into(),
        file: Some(file.trim_start_matches("./").to_string()),
        line: Some(num),
        message: msg.to_string(),
        hints: vec![],
    })
}

#[cfg(not(feature = "typst"))]
fn parse_typst_cli(log: &str, _root: &Path) -> Vec<Diagnostic> {
    let mut out = vec![];
    let lines: Vec<&str> = log.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        let (sev, msg) = if let Some(m) = l.strip_prefix("error: ") {
            ("error", m)
        } else if let Some(m) = l.strip_prefix("warning: ") {
            ("warning", m)
        } else {
            continue;
        };
        // "   ┌─ main.typ:3:5"
        let loc = lines
            .get(i + 1)
            .and_then(|n| n.split("─ ").nth(1))
            .map(str::trim);
        let (file, line) = match loc.and_then(|l| l.split_once(':')) {
            Some((f, rest)) => (
                Some(f.to_string()),
                rest.split(':').next().and_then(|n| n.parse().ok()),
            ),
            None => (None, None),
        };
        out.push(Diagnostic {
            severity: sev.into(),
            file,
            line,
            message: msg.into(),
            hints: vec![],
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_latex_errors() {
        let log = "./main.tex:12: Undefined control sequence.\n! Missing $ inserted.\n<inserted text>\nl.40 The value $x\nLaTeX Warning: Citation `foo' on page 1 undefined on input line 33.\n";
        let d = parse_latex_log(log, Path::new("."));
        assert_eq!(d[0].file.as_deref(), Some("main.tex"));
        assert_eq!(d[0].line, Some(12));
        assert_eq!(d[1].line, Some(40));
        assert_eq!(d[2].severity, "warning");
        assert_eq!(d[2].line, Some(33));
    }

    #[test]
    fn app_bin_is_searched_before_path() {
        let dirs = tool_search_dirs(
            PathBuf::from("/app/bin"),
            Some(PathBuf::from("/home/ada")),
            [PathBuf::from("/usr/bin")],
        );
        assert_eq!(dirs[0], PathBuf::from("/app/bin"));
        assert!(dirs.iter().any(|d| d.ends_with(".local/bin")));
        assert_eq!(dirs.last().unwrap(), &PathBuf::from("/usr/bin"));
    }

    #[cfg(feature = "typst")]
    #[test]
    fn compiles_typst_from_memory() {
        let dir = std::env::temp_dir().join(format!("paper-compile-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut sources = HashMap::new();
        sources.insert(
            "main.typ".to_string(),
            "= Hello\nThis is *live*. #include \"sec.typ\"".to_string(),
        );
        sources.insert("sec.typ".to_string(), "Included text.".to_string());
        let mut c = Compiler::new();
        let out = c.compile(
            &dir,
            "main.typ",
            &sources,
            CompileOptions {
                svg: true,
                pdf: true,
            },
        );
        assert!(out.ok, "{:?}", out.diagnostics);
        assert_eq!(out.page_count, 1);
        assert!(out.svg_pages[0].contains("<svg"));
        assert!(out.pdf.as_ref().unwrap().starts_with(b"%PDF"));

        sources.insert(
            "main.typ".to_string(),
            "= Hello\n#undefined-thing".to_string(),
        );
        let bad = c.compile(&dir, "main.typ", &sources, CompileOptions::default());
        assert!(!bad.ok);
        assert_eq!(bad.diagnostics[0].line, Some(2));
        assert_eq!(bad.diagnostics[0].file.as_deref(), Some("main.typ"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
