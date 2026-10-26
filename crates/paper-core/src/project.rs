use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub const MANIFEST: &str = "paper.toml";
pub const STATE_DIR: &str = ".paper";

/// File extensions tracked as live, versioned documents.
pub const DEFAULT_TRACKED: &[&str] = &["tex", "typ", "bib", "md", "sty", "cls"];
/// Files that are about the project rather than part of the paper.
pub const IGNORED_FILES: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    "README.md",
    "CHANGELOG.md",
    "LICENSE.md",
];
/// Directories never scanned for documents.
pub const IGNORED_DIRS: &[&str] = &[
    ".paper",
    ".git",
    "target",
    "node_modules",
    "build",
    "out",
    "_minted",
    ".venv",
];

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Latex,
    Typst,
}

impl Format {
    pub fn from_path(path: &str) -> Option<Self> {
        match Path::new(path).extension()?.to_str()? {
            "tex" => Some(Format::Latex),
            "typ" => Some(Format::Typst),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub paper: PaperMeta,
    #[serde(default)]
    pub skills: SkillsConfig,
    #[serde(default)]
    pub track: TrackConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaperMeta {
    pub title: String,
    pub format: Format,
    /// Entry point compiled for preview, relative to the project root.
    pub main: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub venue: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
}

/// Which skills agents should apply, globally and per section.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SkillsConfig {
    /// Skills applied to every section.
    #[serde(default)]
    pub global: Vec<String>,
    /// Section title (case-insensitive glob, e.g. `"intro*"`) → skills.
    #[serde(default)]
    pub sections: BTreeMap<String, Vec<String>>,
    /// Skills never applied automatically through their own `sections:` hints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled: Vec<String>,
    /// Turn off automatic matching of skills to sections by their frontmatter.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_auto: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrackConfig {
    #[serde(default = "default_tracked")]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub ignore: Vec<String>,
}

impl Default for TrackConfig {
    fn default() -> Self {
        TrackConfig {
            extensions: default_tracked(),
            ignore: vec![],
        }
    }
}

fn default_tracked() -> Vec<String> {
    DEFAULT_TRACKED.iter().map(|s| s.to_string()).collect()
}

#[derive(Clone, Debug)]
pub struct Project {
    pub root: PathBuf,
    pub manifest: Manifest,
}

impl Project {
    /// Find the project containing `start` by walking up to the nearest `paper.toml`.
    pub fn discover(start: &Path) -> Result<Self> {
        let start = dunce::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
        let mut dir = Some(start.as_path());
        while let Some(d) = dir {
            if d.join(MANIFEST).is_file() {
                return Self::open(d);
            }
            dir = d.parent();
        }
        Err(Error::NoProject(start))
    }

    pub fn open(root: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(root.join(MANIFEST))
            .map_err(|_| Error::NoProject(root.to_path_buf()))?;
        let manifest: Manifest =
            toml::from_str(&text).map_err(|e| Error::Manifest(e.to_string()))?;
        let root = dunce::canonicalize(root)?;
        std::fs::create_dir_all(root.join(STATE_DIR))?;
        Ok(Project { root, manifest })
    }

    /// Create a new paper project. Existing sources are kept; a starter main
    /// file and bibliography are written only when absent.
    pub fn init(root: &Path, title: &str, format: Format) -> Result<Self> {
        std::fs::create_dir_all(root)?;
        if root.join(MANIFEST).exists() {
            return Self::open(root);
        }
        let main = match format {
            Format::Latex => "main.tex",
            Format::Typst => "main.typ",
        };
        let mut sections = BTreeMap::new();
        sections.insert("abstract".into(), vec!["paper-abstract".into()]);
        let manifest = Manifest {
            paper: PaperMeta {
                title: title.into(),
                format,
                main: main.into(),
                venue: None,
                authors: vec![],
            },
            skills: SkillsConfig {
                global: vec![
                    "clarity-editing".into(),
                    match format {
                        Format::Latex => "latex-hygiene".into(),
                        Format::Typst => "typst-writing".into(),
                    },
                ],
                sections,
                ..Default::default()
            },
            track: TrackConfig::default(),
        };
        let project = Project {
            root: root.to_path_buf(),
            manifest,
        };
        project.save_manifest()?;
        std::fs::create_dir_all(root.join(STATE_DIR).join("skills"))?;

        let main_path = root.join(main);
        if !main_path.exists() {
            let body = match format {
                Format::Latex => LATEX_TEMPLATE.replace("{{title}}", title),
                Format::Typst => TYPST_TEMPLATE.replace("{{title}}", title),
            };
            std::fs::write(main_path, body)?;
        }
        let bib = root.join("refs.bib");
        if !bib.exists() {
            std::fs::write(bib, BIB_TEMPLATE)?;
        }
        let gitignore = root.join(".gitignore");
        if !gitignore.exists() {
            std::fs::write(gitignore, GITIGNORE_TEMPLATE)?;
        }
        Self::open(root)
    }

    pub fn save_manifest(&self) -> Result<()> {
        let text =
            toml::to_string_pretty(&self.manifest).map_err(|e| Error::Manifest(e.to_string()))?;
        std::fs::write(self.root.join(MANIFEST), text)?;
        Ok(())
    }

    pub fn state_dir(&self) -> PathBuf {
        self.root.join(STATE_DIR)
    }

    pub fn db_path(&self) -> PathBuf {
        self.state_dir().join("history.db")
    }

    pub fn skills_dir(&self) -> PathBuf {
        self.state_dir().join("skills")
    }

    /// Absolute path of a project-relative document path, rejecting escapes.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf> {
        let rel = normalize_rel(rel)?;
        Ok(self.root.join(rel))
    }

    pub fn is_tracked_path(&self, rel: &str) -> bool {
        let ext = Path::new(rel)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        if !self.manifest.track.extensions.iter().any(|e| e == ext) {
            return false;
        }
        let ignored_by_config = self.manifest.track.ignore.iter().any(|pat| {
            globset::Glob::new(pat)
                .map(|g| g.compile_matcher().is_match(rel))
                .unwrap_or(false)
        });
        !ignored_by_config
            && !IGNORED_FILES.contains(&rel)
            && !rel
                .split('/')
                .any(|seg| IGNORED_DIRS.contains(&seg) || seg.starts_with('.'))
    }

    /// Relative paths of all trackable files currently on disk.
    pub fn scan_files(&self) -> Vec<String> {
        let mut out = vec![];
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if path.is_dir() {
                    if !IGNORED_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                        stack.push(path);
                    }
                } else if let Some(rel) = self.relative(&path) {
                    if self.is_tracked_path(&rel) {
                        out.push(rel);
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// Project-relative, `/`-separated form of an absolute path inside the project.
    pub fn relative(&self, abs: &Path) -> Option<String> {
        let rel = abs.strip_prefix(&self.root).ok()?;
        let parts: Vec<_> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        Some(parts.join("/"))
    }
}

/// Normalise a user-supplied relative path to `a/b/c.tex` form.
pub fn normalize_rel(rel: &str) -> Result<String> {
    let unified = rel.replace('\\', "/");
    let mut parts = vec![];
    for seg in unified.split('/') {
        match seg {
            "" | "." => {}
            ".." => return Err(Error::Invalid(format!("path escapes the project: {rel}"))),
            s => parts.push(s),
        }
    }
    if parts.is_empty() || Path::new(rel).is_absolute() {
        return Err(Error::Invalid(format!("invalid document path: {rel}")));
    }
    Ok(parts.join("/"))
}

const LATEX_TEMPLATE: &str = r#"\documentclass[11pt]{article}
\usepackage[utf8]{inputenc}
\usepackage{amsmath,amssymb}
\usepackage{graphicx}
\usepackage{booktabs}
\usepackage[hidelinks]{hyperref}
\usepackage{cleveref}

\title{{{title}}}
\author{}
\date{}

\begin{document}
\maketitle

\begin{abstract}
TODO: context, gap, approach, key result, implication.
\end{abstract}

\section{Introduction}
\label{sec:intro}

\section{Related Work}
\label{sec:related}

\section{Method}
\label{sec:method}

\section{Experiments}
\label{sec:experiments}

\section{Discussion}
\label{sec:discussion}

\section{Conclusion}
\label{sec:conclusion}

\bibliographystyle{plain}
\bibliography{refs}

\end{document}
"#;

const TYPST_TEMPLATE: &str = r#"#set document(title: "{{title}}")
#set page(paper: "us-letter", margin: 1in, numbering: "1")
#set text(size: 11pt)
#set heading(numbering: "1.")
#set par(justify: true)

#align(center, text(17pt, weight: "bold")[{{title}}])

#heading(level: 1, numbering: none)[Abstract]
TODO: context, gap, approach, key result, implication.

= Introduction <sec:intro>

= Related Work <sec:related>

= Method <sec:method>

= Experiments <sec:experiments>

= Discussion <sec:discussion>

= Conclusion <sec:conclusion>

#bibliography("refs.bib")
"#;

const BIB_TEMPLATE: &str = "% Bibliography. Agents must only cite keys that exist here.\n";

const GITIGNORE_TEMPLATE: &str = "# LaTeX build artefacts\n*.aux\n*.log\n*.out\n*.toc\n*.bbl\n*.blg\n*.fls\n*.fdb_latexmk\n*.synctex.gz\n# LetsWritePaper runtime files\n.paper/owner.lock\n.paper/remote.json\n.paper/history.db-wal\n.paper/history.db-shm\n.paper/build/\n.paper/attachments/\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let p = Project::init(dir.path(), "My Paper", Format::Typst).unwrap();
        assert!(p.root.join("main.typ").exists());
        let p2 = Project::discover(&p.root.join(".paper")).unwrap();
        assert_eq!(p2.manifest.paper.title, "My Paper");
        assert_eq!(p2.manifest.skills.global[1], "typst-writing");
        assert!(p.scan_files().contains(&"main.typ".to_string()));
    }

    #[test]
    fn rejects_escapes() {
        assert!(normalize_rel("../x.tex").is_err());
        assert_eq!(normalize_rel("./sec//intro.tex").unwrap(), "sec/intro.tex");
    }
}
