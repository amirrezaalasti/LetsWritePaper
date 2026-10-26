//! Wiring a project up for AI agents: MCP server registration, the Claude Code
//! hook that attributes direct file edits, and agent-facing instructions.

use std::path::Path;

use serde_json::{json, Value};

use crate::author::Author;
use crate::project::Project;
use crate::workspace::{Ownership, Workspace};
use crate::Result;

/// How to launch this installation's MCP server / hook entry points.
pub struct Launcher<'a> {
    /// Executable that understands `mcp` and `ingest --from-hook` subcommands.
    pub exe: &'a Path,
}

/// Write `.mcp.json`, `.claude/settings.json` (PostToolUse hook), `AGENTS.md`
/// and `CLAUDE.md`. Existing settings are merged, never clobbered.
pub fn write_agent_config(project: &Project, launcher: &Launcher) -> Result<Vec<String>> {
    let exe = launcher.exe.to_string_lossy().to_string();
    let root = &project.root;
    let mut written = vec![];

    let mcp_path = root.join(".mcp.json");
    let mut mcp: Value = read_json(&mcp_path).unwrap_or_else(|| json!({ "mcpServers": {} }));
    mcp["mcpServers"]["paper"] = json!({ "command": exe, "args": ["mcp"] });
    std::fs::write(&mcp_path, serde_json::to_string_pretty(&mcp)? + "\n")?;
    written.push(".mcp.json".to_string());

    // Claude Code also edits files with its own tools; this hook records those
    // edits under the agent's name instead of as anonymous external changes.
    let settings_path = root.join(".claude").join("settings.json");
    std::fs::create_dir_all(root.join(".claude"))?;
    let mut settings: Value = read_json(&settings_path).unwrap_or_else(|| json!({}));
    let hook_cmd = format!("\"{exe}\" ingest --from-hook --author agent:claude-code");
    let mut hooks = settings["hooks"]["PostToolUse"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    hooks.retain(|h| !h.to_string().contains("ingest --from-hook"));
    hooks.push(json!({
        "matcher": "Edit|Write|MultiEdit",
        "hooks": [{ "type": "command", "command": hook_cmd }]
    }));
    settings["hooks"]["PostToolUse"] = Value::Array(hooks);
    std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings)? + "\n",
    )?;
    written.push(".claude/settings.json".to_string());

    if !root.join("AGENTS.md").exists() {
        std::fs::write(root.join("AGENTS.md"), AGENTS_MD)?;
        written.push("AGENTS.md".to_string());
    }
    if !root.join("CLAUDE.md").exists() {
        std::fs::write(root.join("CLAUDE.md"), "@AGENTS.md\n")?;
        written.push("CLAUDE.md".to_string());
    }
    Ok(written)
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Handle a Claude Code PostToolUse hook payload: if the edited file belongs
/// to a paper project, record its new content as an edit by `author`.
/// Returns a short message for the hook's stderr.
pub fn ingest_from_hook(payload: &str, author: &Author) -> Result<Option<String>> {
    let Ok(v) = serde_json::from_str::<Value>(payload) else {
        return Ok(None);
    };
    let Some(file) = v
        .pointer("/tool_input/file_path")
        .or_else(|| v.pointer("/tool_input/path"))
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    let cwd = v
        .get("cwd")
        .and_then(Value::as_str)
        .map(std::path::PathBuf::from);
    let abs = match (Path::new(file).is_absolute(), cwd) {
        (true, _) => Path::new(file).to_path_buf(),
        (false, Some(c)) => c.join(file),
        (false, None) => std::env::current_dir()?.join(file),
    };
    let abs = abs.canonicalize().unwrap_or(abs);
    let Some(dir) = abs.parent() else {
        return Ok(None);
    };
    let Ok(project) = Project::discover(dir) else {
        return Ok(None);
    };
    let Some(rel) = project.relative(&abs) else {
        return Ok(None);
    };
    if !project.is_tracked_path(&rel) {
        return Ok(None);
    }
    let mut ws = Workspace::open(project, Ownership::Never)?;
    ws.set_session(format!("hook:{}", author.id));
    Ok(ws
        .ingest_file(&rel, author, Some("edited via agent file tool"))?
        .map(|id| format!("paper: recorded {rel} as #{id} by {}", author.name)))
}

pub const AGENTS_MD: &str = r#"# Working on this paper

This is a LetsWritePaper project. Humans and agents edit it together, and every change is
recorded with its author. Prefer the `paper` MCP server tools over editing files directly:

1. `project_overview` — structure, skills, open reviews.
2. `section_context {section}` before editing a section — it returns the skills to follow
   and any open reviewer comments.
3. `edit_document {path, old_text, new_text, rationale}` for focused edits;
   `propose_edit` for substantive rewrites unless asked to apply them.
4. `compile` to check the paper still builds.
5. Never invent citations: only cite keys that exist in the .bib files; otherwise write TODO.

Peer review: `start_review` → `add_review_comment` (exact quotes) → `finish_review`.
Addressing reviews: `list_review_comments {status: "open"}` → edit → `respond_to_comment`.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_attributes_agent_edits() {
        std::env::set_var("PAPER_NO_CLAUDE_SKILLS", "1");
        let dir = tempfile::tempdir().unwrap();
        let project = Project::init(dir.path(), "Hooked", crate::Format::Latex).unwrap();
        drop(Workspace::open(project.clone(), Ownership::Never).unwrap());
        // First open as owner imports files.
        drop(Workspace::open(project.clone(), Ownership::Auto).unwrap());
        let main = project.root.join("main.tex");
        let text = std::fs::read_to_string(&main)
            .unwrap()
            .replace("\\section{Method}", "\\section{Approach}");
        std::fs::write(&main, text).unwrap();
        let payload =
            json!({ "tool_name": "Edit", "tool_input": { "file_path": main.to_string_lossy() } })
                .to_string();
        let msg = ingest_from_hook(&payload, &Author::agent("claude-code"))
            .unwrap()
            .unwrap();
        assert!(msg.contains("main.tex"));
        let ws = Workspace::open(project, Ownership::Never).unwrap();
        assert_eq!(
            ws.log(Some("main.tex"), 1).unwrap()[0].author.id,
            "agent:claude-code"
        );
        // Unrelated files are ignored.
        assert!(ingest_from_hook(
            r#"{"tool_input":{"file_path":"/tmp/nope.txt"}}"#,
            &Author::agent("x")
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn config_is_merged_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::init(dir.path(), "Cfg", crate::Format::Latex).unwrap();
        std::fs::create_dir_all(project.root.join(".claude")).unwrap();
        std::fs::write(
            project.root.join(".claude/settings.json"),
            r#"{"permissions":{"allow":["Bash(ls)"]}}"#,
        )
        .unwrap();
        let l = Launcher {
            exe: Path::new("/usr/local/bin/paper"),
        };
        write_agent_config(&project, &l).unwrap();
        write_agent_config(&project, &l).unwrap();
        let s: Value = read_json(&project.root.join(".claude/settings.json")).unwrap();
        assert_eq!(s["permissions"]["allow"][0], "Bash(ls)");
        assert_eq!(s["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);
        let m: Value = read_json(&project.root.join(".mcp.json")).unwrap();
        assert_eq!(m["mcpServers"]["paper"]["args"][0], "mcp");
        assert!(!project.is_tracked_path("AGENTS.md"));
    }
}
