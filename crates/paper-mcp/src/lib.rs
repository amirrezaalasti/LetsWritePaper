//! MCP server (JSON-RPC 2.0 over stdio) for LetsWritePaper.
//!
//! Start it with `paper mcp` in a project. Agents identify themselves through
//! the MCP `clientInfo` (or `--agent` / `$PAPER_AGENT`), and every edit they
//! make is attributed to them in the paper's history.

pub mod tools;

use std::io::{BufRead, Write};
use std::path::Path;

use anyhow::Result;
use paper_compile::Compiler;
use paper_core::{Author, Ownership, Project, Workspace};
use serde_json::{json, Value};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "You are co-authoring an academic paper in LetsWritePaper. \
Every edit you make is recorded in the paper's history under your name, and humans may be editing at the same time. \
Workflow: call project_overview first; before changing a section call section_context (it returns the section text, \
the writing skills to follow, and open reviewer comments); make small edit_document calls with exact old_text and a \
rationale; use propose_edit for substantive rewrites unless told to apply directly; compile to check for errors; \
never invent citations — only use keys present in the project's .bib files. For peer review use start_review / \
add_review_comment / finish_review and do not edit the paper.";

pub struct Server {
    ws: Workspace,
    compiler: Compiler,
    author: Author,
    /// Fixed by `--agent`/`$PAPER_AGENT`; otherwise taken from clientInfo.
    author_fixed: bool,
}

impl Server {
    pub fn new(project_root: &Path, agent: Option<&str>) -> Result<Self> {
        let project = Project::discover(project_root)?;
        let ws = Workspace::open(project, Ownership::Auto)?;
        let fixed = agent.map(str::to_string).or_else(|| std::env::var("PAPER_AGENT").ok()).filter(|s| !s.is_empty());
        let author = fixed.as_deref().map(agent_author).unwrap_or_else(|| Author::agent("mcp-agent"));
        let mut server = Server { ws, compiler: Compiler::new(), author, author_fixed: fixed.is_some() };
        server.refresh_session();
        Ok(server)
    }

    fn refresh_session(&mut self) {
        let short = uuid::Uuid::new_v4().to_string()[..8].to_string();
        self.ws.set_session(format!("mcp:{}:{short}", self.author.id));
    }

    /// Handle one JSON-RPC message; returns the response for requests.
    pub fn handle(&mut self, msg: &Value) -> Option<Value> {
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let id = id?; // notifications get no response
        let result = match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools::definitions() })),
            "tools/call" => Ok(self.call_tool(&params)),
            "prompts/list" => Ok(self.prompts_list()),
            "prompts/get" => self.prompts_get(&params),
            "resources/list" => Ok(json!({ "resources": [] })),
            _ => Err((-32601, format!("method not found: {method}"))),
        };
        Some(match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
        })
    }

    fn initialize(&mut self, params: &Value) -> Value {
        if !self.author_fixed {
            if let Some(name) = params.pointer("/clientInfo/name").and_then(Value::as_str) {
                self.author = agent_author(name);
                self.refresh_session();
            }
        }
        let requested = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION);
        let version = if SUPPORTED_VERSIONS.contains(&requested) { requested } else { PROTOCOL_VERSION };
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": { "listChanged": false }, "prompts": { "listChanged": false } },
            "serverInfo": { "name": "letswritepaper", "version": env!("CARGO_PKG_VERSION") },
            "instructions": INSTRUCTIONS,
        })
    }

    fn call_tool(&mut self, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
        let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
        // Pick up concurrent human edits first; take over file mirroring if the app closed.
        let _ = self.ws.try_take_ownership();
        let _ = self.ws.poll();
        let mut ctx = tools::ToolContext { ws: &mut self.ws, author: &self.author, compiler: &mut self.compiler };
        let result = tools::call(&mut ctx, &name, &args);
        self.ws.drain_events();
        match result {
            Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
            Err(e) => json!({ "content": [{ "type": "text", "text": format!("Error: {e:#}") }], "isError": true }),
        }
    }

    fn prompts_list(&self) -> Value {
        let mut prompts: Vec<Value> = vec![
            json!({ "name": "write-section", "description": "Draft or revise a section following the skills bound to it.",
                    "arguments": [{ "name": "section", "description": "Section title", "required": true },
                                  { "name": "instructions", "description": "What to change", "required": false }] }),
            json!({ "name": "review-paper", "description": "Run a structured peer review as a reviewer persona.",
                    "arguments": [{ "name": "persona", "description": "methods-skeptic, clarity-reviewer, novelty-reviewer, reproducibility-reviewer, area-chair", "required": false },
                                  { "name": "venue", "description": "Target venue", "required": false }] }),
            json!({ "name": "address-reviews", "description": "Work through open review comments and record responses.", "arguments": [] }),
        ];
        for s in self.ws.skills.skills.values() {
            prompts.push(json!({
                "name": format!("skill:{}", s.name),
                "description": s.description,
                "arguments": [{ "name": "section", "description": "Section to apply it to", "required": false }],
            }));
        }
        json!({ "prompts": prompts })
    }

    fn prompts_get(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
        let arg = |k: &str| params.pointer(&format!("/arguments/{k}")).and_then(Value::as_str).unwrap_or("").to_string();
        let text = match name {
            "write-section" => format!(
                "Work on the section \"{}\" of this paper. First call section_context with section=\"{}\" and follow every skill it returns. \
                 Then make focused edit_document calls with clear rationales, and compile to check. {}",
                arg("section"), arg("section"), arg("instructions")
            ),
            "review-paper" => {
                let persona = if arg("persona").is_empty() { "methods-skeptic".into() } else { arg("persona") };
                let venue = arg("venue");
                format!(
                    "Peer-review this paper as the `{persona}` persona{}. Call get_skill name=\"peer-review\" and follow it: \
                     project_overview → read the paper → start_review → add_review_comment for each issue (exact quotes) → finish_review with scores. Do not edit the paper.",
                    if venue.is_empty() { String::new() } else { format!(" for {venue}") }
                )
            }
            "address-reviews" => "Call get_skill name=\"rebuttal\" and follow it to address every open review comment (list_review_comments status=open), \
                editing or proposing changes and recording a response for each with respond_to_comment."
                .to_string(),
            n if n.starts_with("skill:") => {
                let skill = self.ws.skills.get(&n[6..]).ok_or((-32602, format!("unknown prompt {n}")))?;
                let section = arg("section");
                format!(
                    "{}\n\n---\nApply the skill above{}. Use section_context, then edit_document/propose_edit.",
                    skill.body,
                    if section.is_empty() { String::new() } else { format!(" to the section \"{section}\"") }
                )
            }
            _ => return Err((-32602, format!("unknown prompt {name}"))),
        };
        Ok(json!({ "messages": [{ "role": "user", "content": { "type": "text", "text": text } }] }))
    }

    pub fn author(&self) -> &Author {
        &self.author
    }
}

/// Normalise MCP client names ("claude-code", "Claude Desktop", "cursor-vscode").
fn agent_author(name: &str) -> Author {
    Author::agent(name.trim_start_matches("agent:"))
}

/// Serve MCP over stdio until stdin closes.
pub fn serve_stdio(project_root: &Path, agent: Option<&str>) -> Result<()> {
    let mut server = Server::new(project_root, agent)?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                let out: Vec<Value> = batch.iter().filter_map(|m| server.handle(m)).collect();
                (!out.is_empty()).then_some(Value::Array(out))
            }
            Ok(msg) => server.handle(&msg),
            Err(e) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": e.to_string() } })),
        };
        if let Some(r) = response {
            writeln!(stdout, "{r}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(server: &mut Server, id: i64, name: &str, args: Value) -> (String, bool) {
        let r = server
            .handle(&json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": args } }))
            .unwrap();
        let text = r.pointer("/result/content/0/text").and_then(Value::as_str).unwrap().to_string();
        let err = r.pointer("/result/isError").and_then(Value::as_bool).unwrap_or(false);
        (text, err)
    }

    #[test]
    fn agent_session_end_to_end() {
        std::env::set_var("PAPER_NO_CLAUDE_SKILLS", "1");
        let dir = tempfile::tempdir().unwrap();
        Project::init(dir.path(), "MCP Test", paper_core::Format::Typst).unwrap();
        let mut s = Server::new(dir.path(), None).unwrap();
        let init = s
            .handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "2025-06-18", "clientInfo": { "name": "claude-code", "version": "1" }, "capabilities": {} } }))
            .unwrap();
        assert_eq!(init.pointer("/result/serverInfo/name").unwrap(), "letswritepaper");
        assert_eq!(s.author().id, "agent:claude-code");
        assert!(s.handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).is_none());

        let list = s.handle(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })).unwrap();
        assert!(list.pointer("/result/tools").unwrap().as_array().unwrap().len() > 20);

        let (ctx, _) = call(&mut s, 3, "section_context", json!({ "section": "Introduction" }));
        assert!(ctx.contains("Skill: paper-introduction"), "{ctx}");

        let (out, err) = call(&mut s, 4, "edit_document", json!({
            "path": "main.typ", "old_text": "= Introduction <sec:intro>\n",
            "new_text": "= Introduction <sec:intro>\nTraining large models is costly.\n", "rationale": "first sentence" }));
        assert!(!err, "{out}");
        let (blame, _) = call(&mut s, 5, "blame", json!({ "path": "main.typ" }));
        assert!(blame.lines().any(|l| l.contains("claude-code (agent)") && l.contains("Training large models")), "{blame}");

        let (out, err) = call(&mut s, 6, "edit_document", json!({ "path": "main.typ", "old_text": "nope", "new_text": "x", "rationale": "r" }));
        assert!(err && out.contains("text not found"));
        let (out, err) = call(&mut s, 61, "edit_document", json!({ "path": "main.typ", "old_text": "Training large models is costly.\n\n\n", "new_text": "x", "rationale": "r" }));
        assert!(err && out.contains("actually contains") && out.contains("Training large models is costly."), "{out}");

        let (out, _) = call(&mut s, 7, "start_review", json!({ "title": "Mock", "persona": "clarity-reviewer" }));
        assert!(out.contains("round_id = 1"));
        let (out, err) = call(&mut s, 8, "add_review_comment", json!({
            "round_id": 1, "path": "main.typ", "quote": "Training large models is costly.",
            "severity": "minor", "category": "clarity", "comment": "Quantify the cost." }));
        assert!(!err, "{out}");
        assert!(out.contains("Introduction"));
        let (out, _) = call(&mut s, 9, "compile", json!({}));
        assert!(out.starts_with("Compiled with typst"), "{out}");
        let (out, _) = call(&mut s, 10, "history", json!({}));
        assert!(out.contains("first sentence"));

        let prompts = s.handle(&json!({ "jsonrpc": "2.0", "id": 11, "method": "prompts/get", "params": { "name": "skill:peer-review" } })).unwrap();
        assert!(prompts.pointer("/result/messages/0/content/text").is_some());
    }
}
