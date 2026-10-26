use serde::{Deserialize, Serialize};

/// Who made a change. Agents and humans are first-class so that history,
/// blame and review can always answer "was this written by a model?".
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AuthorKind {
    Human,
    Agent,
    /// A change discovered on disk whose origin is unknown (another editor, a script).
    External,
    /// Imports, merges and other bookkeeping done by the tool itself.
    System,
}

impl AuthorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthorKind::Human => "human",
            AuthorKind::Agent => "agent",
            AuthorKind::External => "external",
            AuthorKind::System => "system",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "human" => AuthorKind::Human,
            "agent" => AuthorKind::Agent,
            "system" => AuthorKind::System,
            _ => AuthorKind::External,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Author {
    /// Stable identifier such as `human:ada` or `agent:claude-code`.
    pub id: String,
    pub kind: AuthorKind,
    /// Display name.
    pub name: String,
}

impl Author {
    pub fn new(kind: AuthorKind, name: &str) -> Self {
        Author {
            id: format!("{}:{}", kind.as_str(), slug(name)),
            kind,
            name: name.to_string(),
        }
    }

    pub fn human(name: &str) -> Self {
        Self::new(AuthorKind::Human, name)
    }

    pub fn agent(name: &str) -> Self {
        Self::new(AuthorKind::Agent, name)
    }

    pub fn external() -> Self {
        Self::new(AuthorKind::External, "filesystem")
    }

    pub fn system() -> Self {
        Self::new(AuthorKind::System, "letswritepaper")
    }

    /// Parse `kind:name` (e.g. `agent:claude-code`); a bare name is a human.
    pub fn parse(spec: &str) -> Self {
        match spec.split_once(':') {
            Some((kind, name)) => Self::new(AuthorKind::parse(kind), name),
            None => Self::human(spec),
        }
    }

    /// The local human user: `$PAPER_AUTHOR`, then the name saved by the setup
    /// wizard, then git's `user.name`, then the OS user.
    pub fn local_user() -> Self {
        if let Ok(spec) = std::env::var("PAPER_AUTHOR") {
            if !spec.trim().is_empty() {
                return Self::parse(spec.trim());
            }
        }
        let saved = crate::user_config::load().name;
        if !saved.trim().is_empty() {
            return Self::human(saved.trim());
        }
        let git_name = std::process::Command::new("git")
            .args(["config", "user.name"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());
        let os_user = std::env::var("USER")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("USERNAME").ok().filter(|s| !s.is_empty()));
        Self::human(&fallback_name(
            None,
            git_name.as_deref(),
            os_user.as_deref(),
        ))
    }
}

/// First non-empty candidate, otherwise `"author"`.
fn fallback_name(saved: Option<&str>, git: Option<&str>, os_user: Option<&str>) -> String {
    for candidate in [saved, git, os_user] {
        if let Some(name) = candidate.map(str::trim).filter(|s| !s.is_empty()) {
            return name.to_string();
        }
    }
    "author".into()
}

pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "anonymous".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_specs() {
        let a = Author::parse("agent:Claude Code");
        assert_eq!(a.id, "agent:claude-code");
        assert_eq!(a.kind, AuthorKind::Agent);
        assert_eq!(Author::parse("Ada Lovelace").id, "human:ada-lovelace");
    }

    #[test]
    fn fallback_name_prefers_the_saved_profile() {
        assert_eq!(fallback_name(Some("Ada"), Some("Git"), Some("os")), "Ada");
        assert_eq!(fallback_name(Some("  "), Some("Git"), Some("os")), "Git");
        assert_eq!(fallback_name(None, None, Some("os")), "os");
        assert_eq!(fallback_name(None, None, None), "author");
    }
}
