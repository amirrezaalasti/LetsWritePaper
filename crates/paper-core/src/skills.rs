//! Skills: Markdown instruction files (`SKILL.md`, the Claude Code / Agent
//! Skills format) that tell agents how to write or review a given section.
//!
//! Sources, highest precedence first:
//! 1. project — `.paper/skills/<name>/SKILL.md` (versioned with the paper)
//! 2. user — `<config dir>/letswritepaper/skills/<name>/SKILL.md`
//! 3. claude — `~/.claude/skills/<name>/SKILL.md` (skills already installed for Claude Code)
//! 4. bundled — shipped with the app
//!
//! A skill is attached to a section through `paper.toml` (`[skills] global` and
//! `[skills.sections]`) or automatically through its own `sections:` globs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::project::{Project, SkillsConfig};
use crate::{Error, Result};

mod bundled {
    include!(concat!(env!("OUT_DIR"), "/bundled_skills.rs"));
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    Project,
    User,
    Claude,
    Bundled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// `writing`, `review` or `style`.
    pub kind: String,
    /// Lowercase globs matched against section titles.
    pub sections: Vec<String>,
    pub body: String,
    pub source: SkillSource,
    pub path: Option<PathBuf>,
}

/// Why a skill applies to a section.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppliedSkill {
    pub name: String,
    pub description: String,
    pub reason: String,
}

/// Parse a `SKILL.md`: YAML-ish frontmatter (scalars, quoted strings, inline
/// and block lists, folded/literal block scalars) followed by a Markdown body.
pub fn parse_skill_md(
    text: &str,
    fallback_name: &str,
    source: SkillSource,
    path: Option<PathBuf>,
) -> Skill {
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    let mut lists: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut body = text;
    let normalized = text.trim_start_matches('\u{feff}');
    if let Some(rest) = normalized.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            let front = &rest[..end];
            body = rest[end + 4..]
                .trim_start_matches(['-', '\r'])
                .trim_start_matches('\n');
            let lines: Vec<&str> = front.lines().collect();
            let mut i = 0;
            while i < lines.len() {
                let line = lines[i];
                i += 1;
                if line.starts_with([' ', '\t'])
                    || line.trim().is_empty()
                    || line.trim_start().starts_with('#')
                {
                    continue;
                }
                let Some((key, value)) = line.split_once(':') else {
                    continue;
                };
                let key = key.trim().to_string();
                let value = value.trim();
                // Indented continuation lines belong to this key.
                let mut cont = vec![];
                while i < lines.len()
                    && (lines[i].starts_with([' ', '\t']) || lines[i].trim().is_empty())
                {
                    cont.push(lines[i].trim());
                    i += 1;
                }
                if value.starts_with('[') {
                    let inner = value.trim_start_matches('[').trim_end_matches(']');
                    lists.insert(
                        key,
                        inner
                            .split(',')
                            .map(unquote)
                            .filter(|s| !s.is_empty())
                            .collect(),
                    );
                } else if value.is_empty() && cont.iter().any(|c| c.starts_with('-')) {
                    lists.insert(
                        key,
                        cont.iter()
                            .filter_map(|c| c.strip_prefix('-'))
                            .map(unquote)
                            .filter(|s| !s.is_empty())
                            .collect(),
                    );
                } else if value.starts_with('>') || value.starts_with('|') {
                    let sep = if value.starts_with('>') { " " } else { "\n" };
                    fields.insert(key, cont.join(sep).trim().to_string());
                } else {
                    let mut v = unquote(value);
                    if !cont.is_empty() {
                        v = format!("{v} {}", cont.join(" ")).trim().to_string();
                    }
                    fields.insert(key, v);
                }
            }
        }
    }
    Skill {
        name: fields
            .remove("name")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| fallback_name.to_string()),
        description: fields.remove("description").unwrap_or_default(),
        kind: fields.remove("kind").unwrap_or_else(|| "writing".into()),
        sections: lists
            .remove("sections")
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.to_lowercase())
            .collect(),
        body: body.trim().to_string(),
        source,
        path,
    }
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .or_else(|| s.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')))
        .unwrap_or(s);
    s.trim().to_string()
}

pub fn user_skills_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("letswritepaper").join("skills"))
}

pub fn claude_skills_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|d| d.join(".claude").join("skills"))
}

fn load_dir(dir: &Path, source: SkillSource) -> Vec<Skill> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut out = vec![];
    for entry in rd.flatten() {
        let md = entry.path().join("SKILL.md");
        if let Ok(text) = std::fs::read_to_string(&md) {
            let fallback = entry.file_name().to_string_lossy().to_string();
            out.push(parse_skill_md(&text, &fallback, source, Some(md)));
        }
    }
    out
}

#[derive(Clone, Debug, Default)]
pub struct SkillRegistry {
    pub skills: BTreeMap<String, Skill>,
}

impl SkillRegistry {
    /// Load all sources for a project (or just user/claude/bundled without one).
    pub fn load(project: Option<&Project>) -> Self {
        let mut reg = SkillRegistry::default();
        for (name, text) in bundled::BUNDLED {
            reg.insert(parse_skill_md(text, name, SkillSource::Bundled, None));
        }
        if std::env::var_os("PAPER_NO_CLAUDE_SKILLS").is_none() {
            if let Some(dir) = claude_skills_dir() {
                load_dir(&dir, SkillSource::Claude)
                    .into_iter()
                    .for_each(|s| reg.insert(s));
            }
        }
        if let Some(dir) = user_skills_dir() {
            load_dir(&dir, SkillSource::User)
                .into_iter()
                .for_each(|s| reg.insert(s));
        }
        if let Some(p) = project {
            load_dir(&p.skills_dir(), SkillSource::Project)
                .into_iter()
                .for_each(|s| reg.insert(s));
        }
        reg
    }

    /// Later inserts override earlier ones; an override without `sections:`
    /// inherits the section globs of the skill it replaces.
    fn insert(&mut self, mut skill: Skill) {
        if let Some(prev) = self.skills.get(&skill.name) {
            if skill.sections.is_empty() {
                skill.sections = prev.sections.clone();
            }
            if skill.kind == "writing" && prev.kind != "writing" {
                skill.kind = prev.kind.clone();
            }
        }
        self.skills.insert(skill.name.clone(), skill);
    }

    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    /// Skills applicable to a section (or to the whole paper when `section` is None).
    pub fn for_section(&self, cfg: &SkillsConfig, section: Option<&str>) -> Vec<AppliedSkill> {
        let mut out: Vec<AppliedSkill> = vec![];
        let push = |name: &str, reason: String, out: &mut Vec<AppliedSkill>| {
            if out.iter().any(|a| a.name == name) {
                return;
            }
            let description = self
                .get(name)
                .map(|s| s.description.clone())
                .unwrap_or_else(|| "(skill not installed)".into());
            out.push(AppliedSkill {
                name: name.into(),
                description,
                reason,
            });
        };
        for name in &cfg.global {
            push(name, "global in paper.toml".into(), &mut out);
        }
        let Some(title) = section.map(|s| s.trim().to_lowercase()) else {
            return out;
        };
        for (pattern, names) in &cfg.sections {
            if glob_match(&pattern.to_lowercase(), &title) {
                for name in names {
                    push(
                        name,
                        format!("paper.toml binds section {pattern:?}"),
                        &mut out,
                    );
                }
            }
        }
        if !cfg.no_auto {
            for skill in self.skills.values() {
                if cfg.disabled.contains(&skill.name) {
                    continue;
                }
                if let Some(p) = skill.sections.iter().find(|p| glob_match(p, &title)) {
                    push(
                        &skill.name,
                        format!("matches its section pattern {p:?}"),
                        &mut out,
                    );
                }
            }
        }
        out
    }
}

/// Match a section title against a glob, also trying each word-boundary
/// suffix so `method*` matches "Our Method" and "Proposed Methodology".
fn glob_match(pattern: &str, title: &str) -> bool {
    let Ok(glob) = globset::GlobBuilder::new(pattern)
        .case_insensitive(true)
        .literal_separator(false)
        .build()
    else {
        return false;
    };
    let m = glob.compile_matcher();
    let title = title.trim();
    m.is_match(title)
        || title
            .char_indices()
            .filter(|(i, c)| *c == ' ' && *i + 1 < title.len())
            .any(|(i, _)| m.is_match(&title[i + 1..]))
}

/// Copy a skill directory (containing `SKILL.md`) into the project, or into the
/// user directory when `project` is None. Returns the installed skill name.
pub fn install_skill(src: &Path, project: Option<&Project>) -> Result<String> {
    let src_md = if src.is_file() {
        src.to_path_buf()
    } else {
        src.join("SKILL.md")
    };
    let text = std::fs::read_to_string(&src_md)
        .map_err(|_| Error::Invalid(format!("no SKILL.md found at {}", src.display())))?;
    let src_dir = src_md.parent().unwrap_or(src).to_path_buf();
    let fallback = src_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "skill".into());
    let skill = parse_skill_md(&text, &fallback, SkillSource::Project, None);
    let name = crate::author::slug(&skill.name);
    let base = match project {
        Some(p) => p.skills_dir(),
        None => {
            user_skills_dir().ok_or_else(|| Error::Invalid("no user config directory".into()))?
        }
    };
    let dest = base.join(&name);
    std::fs::create_dir_all(&dest)?;
    if src.is_file() {
        std::fs::write(dest.join("SKILL.md"), text)?;
    } else {
        copy_dir(&src_dir, &dest)?;
    }
    Ok(name)
}

/// Write a new skill from scratch into the project.
pub fn create_skill(
    project: &Project,
    name: &str,
    description: &str,
    kind: &str,
    sections: &[String],
    body: &str,
) -> Result<PathBuf> {
    let name = crate::author::slug(name);
    let dir = project.skills_dir().join(&name);
    std::fs::create_dir_all(&dir)?;
    let sections = sections
        .iter()
        .map(|s| format!("{s:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let text = format!(
        "---\nname: {name}\ndescription: {}\nkind: {kind}\nsections: [{sections}]\n---\n\n{}\n",
        description.replace('\n', " "),
        body.trim()
    );
    let path = dir.join("SKILL.md");
    std::fs::write(&path, text)?;
    Ok(path)
}

fn copy_dir(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let to = dest.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_variants() {
        let s = parse_skill_md(
            "---\nname: x\ndescription: >\n  folded\n  text\nsections:\n  - \"intro*\"\n  - method\nallowed-tools: Read\n---\n# Body\n",
            "fallback",
            SkillSource::User,
            None,
        );
        assert_eq!(s.name, "x");
        assert_eq!(s.description, "folded text");
        assert_eq!(s.sections, ["intro*", "method"]);
        assert_eq!(s.body, "# Body");
        let bare = parse_skill_md("Just text", "fb", SkillSource::User, None);
        assert_eq!(bare.name, "fb");
    }

    #[test]
    fn bundled_skills_resolve_for_sections() {
        let reg = SkillRegistry {
            skills: bundled::BUNDLED
                .iter()
                .map(|(n, t)| {
                    (
                        n.to_string(),
                        parse_skill_md(t, n, SkillSource::Bundled, None),
                    )
                })
                .collect(),
        };
        assert!(reg.get("peer-review").is_some());
        let mut cfg = SkillsConfig {
            global: vec!["clarity-editing".into()],
            ..Default::default()
        };
        cfg.sections
            .insert("related*".into(), vec!["my-custom".into()]);
        let names: Vec<_> = reg
            .for_section(&cfg, Some("Introduction"))
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(names, ["clarity-editing", "paper-introduction"]);
        let names: Vec<_> = reg
            .for_section(&cfg, Some("Related Work"))
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert!(
            names.contains(&"my-custom".to_string())
                && names.contains(&"paper-related-work".to_string())
        );
    }
}
