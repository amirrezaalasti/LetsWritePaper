//! Paper-aware tools. Every write is attributed to the calling agent and
//! lands in the project history like any human edit.

use std::collections::HashMap;

use anyhow::{anyhow, bail, Context, Result};
use paper_compile::{CompileOptions, Compiler};
use paper_core::history::{render_diffs, VersionRef};
use paper_core::review::{Scores, CATEGORIES, PERSONAS, SEVERITIES};
use paper_core::store::CommentFilter;
use paper_core::{format_ms, Author, Workspace};
use serde_json::{json, Value};

pub struct ToolContext<'a> {
    pub ws: &'a mut Workspace,
    pub author: &'a Author,
    pub compiler: &'a mut Compiler,
}

fn schema(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required })
}

/// Tool definitions in MCP `tools/list` shape.
pub fn definitions() -> Vec<Value> {
    let s = |d: &str| json!({ "type": "string", "description": d });
    let i = |d: &str| json!({ "type": "integer", "description": d });
    let tools = vec![
        ("project_overview", "Start here. Paper title, venue, format, documents, outline of the main file, checkpoints, skills in force, and counts of open review comments and pending proposals.", schema(json!({}), &[])),
        ("read_document", "Read a document (or one section of it). Copy text verbatim from here into old_text/quote arguments.",
            schema(json!({ "path": s("Project-relative path, e.g. main.tex"), "section": s("Optional section title or label, e.g. 'Introduction' or 'sec:intro'"), "line_numbers": { "type": "boolean", "description": "Prefix lines with numbers (do not copy these into old_text)" } }), &["path"])),
        ("outline", "Section structure (level, title, label, line range) of a document; defaults to the main file.",
            schema(json!({ "path": s("Document path") }), &[])),
        ("search", "Case-insensitive search across all documents. Returns path:line matches.",
            schema(json!({ "query": s("Text to find"), "max_results": i("Default 50") }), &["query"])),
        ("section_context", "Call before editing a section: returns its text, the writing skills that apply to it (with full instructions), open reviewer comments on it, and pending suggested edits.",
            schema(json!({ "section": s("Section title or label"), "path": s("Document (optional; searched if omitted)") }), &["section"])),
        ("edit_document", "Replace an exact, unique excerpt of a document. Prefer several small edits over rewriting files. The rationale is shown in history.",
            schema(json!({ "path": s("Document path"), "old_text": s("Exact existing text (must be unique unless occurrence is given)"), "new_text": s("Replacement text"), "rationale": s("Why — shown to co-authors in history"), "occurrence": i("1-based occurrence when old_text repeats") }), &["path", "old_text", "new_text", "rationale"])),
        ("write_document", "Create a document, or replace a whole document's content (stored as a minimal diff so authorship of unchanged text is kept).",
            schema(json!({ "path": s("Document path, e.g. sections/intro.tex"), "content": s("Full content"), "rationale": s("Why") }), &["path", "content", "rationale"])),
        ("propose_edit", "Suggest an edit for a human to accept or reject instead of applying it. Use for substantive changes or when asked for suggestions.",
            schema(json!({ "path": s("Document path"), "old_text": s("Exact existing text"), "new_text": s("Suggested replacement"), "rationale": s("Why; reference review comment ids if relevant") }), &["path", "old_text", "new_text", "rationale"])),
        ("list_proposals", "Suggested edits and their status (pending, accepted, rejected, stale).",
            schema(json!({ "status": s("Filter: pending | accepted | rejected | stale") }), &[])),
        ("history", "Recent edit sessions: who (human/agent) changed which document, when, how much, and why.",
            schema(json!({ "path": s("Limit to one document"), "limit": i("Default 20") }), &[])),
        ("diff", "Word-level diff grouped by section between two versions. Versions: 'now', a checkpoint name, 'last' (latest checkpoint), 'start', or '#<update id>'.",
            schema(json!({ "from": s("Default: last"), "to": s("Default: now"), "path": s("Limit to one document") }), &[])),
        ("blame", "Who wrote each line (human or agent).",
            schema(json!({ "path": s("Document path"), "start_line": i("First line"), "end_line": i("Last line") }), &["path"])),
        ("checkpoint", "Save a named version of the whole paper (also a git commit when the project is a git repo). Use milestone for submission events.",
            schema(json!({ "name": s("Unique name, e.g. draft-2"), "message": s("What this version is"), "milestone": s("Optional: submitted | rebuttal | camera-ready | arxiv | <custom>") }), &["name", "message"])),
        ("compile", "Compile the paper and return errors/warnings with file and line.",
            schema(json!({ "main": s("Entry file; defaults to paper.toml main") }), &[])),
        ("list_skills", "All available writing/review skills and which sections they apply to.", schema(json!({}), &[])),
        ("get_skill", "Full instructions of a skill.", schema(json!({ "name": s("Skill name") }), &["name"])),
        ("bind_skill", "Attach a skill to sections whose title matches a glob (e.g. 'intro*'), or to the whole paper when section is omitted. Saved in paper.toml.",
            schema(json!({ "skill": s("Skill name"), "section": s("Section title glob") }), &["skill"])),
        ("create_skill", "Create a project skill (SKILL.md in .paper/skills) — e.g. venue style rules or the lab's conventions.",
            schema(json!({ "name": s("kebab-case name"), "description": s("When to use it"), "kind": s("writing | review | style"), "sections": { "type": "array", "items": { "type": "string" }, "description": "Section title globs" }, "body": s("Markdown instructions") }), &["name", "description", "body"])),
        ("start_review", "Begin a peer-review round as a reviewer persona. Returns round_id. Reviewers must not edit the paper.",
            schema(json!({ "title": s("e.g. 'Mock NeurIPS review'"), "persona": s(&format!("One of: {}", PERSONAS.iter().map(|p| p.0).collect::<Vec<_>>().join(", "))), "venue": s("Target venue") }), &["title"])),
        ("add_review_comment", "Add a review comment anchored to an exact quote from the paper.",
            schema(json!({ "round_id": i("From start_review"), "path": s("Document path"), "quote": s("Verbatim excerpt (5-30 words) the comment refers to"), "severity": s(&SEVERITIES.join(" | ")), "category": s(&CATEGORIES.join(" | ")), "comment": s("Specific, actionable feedback; for major issues say what would resolve it"), "section": s("Optional; inferred from the quote") }), &["round_id", "path", "quote", "severity", "category", "comment"])),
        ("finish_review", "Close a review round with a summary and rubric scores.",
            schema(json!({ "round_id": i("Round"), "summary": s("Strengths, weaknesses, decisive issues"), "scores": { "type": "object", "properties": { "soundness": i("1-4"), "presentation": i("1-4"), "contribution": i("1-4"), "overall": i("1-10"), "confidence": i("1-5") } } }), &["round_id", "summary"])),
        ("list_review_comments", "Review comments with their current line (null when the quoted text was since rewritten).",
            schema(json!({ "round_id": i("Filter by round"), "status": s("open | addressed | disputed | wontfix"), "path": s("Filter by document") }), &[])),
        ("respond_to_comment", "Record the authors' response to a review comment.",
            schema(json!({ "comment_id": i("Comment id"), "response": s("What was changed (quote it) or why not"), "status": s("addressed | disputed | wontfix | open") }), &["comment_id", "response", "status"])),
    ];
    tools
        .into_iter()
        .map(|(name, description, input_schema)| json!({ "name": name, "description": description, "inputSchema": input_schema }))
        .collect()
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| anyhow!("missing string argument `{key}`"))
}

fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).filter(|s| !s.trim().is_empty())
}

fn opt_int(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
}

/// When `old` isn't in `text`, show what the document has where `old` seems to
/// start, so the agent can retry with the exact text.
fn near_miss(text: &str, old: &str) -> String {
    let Some(first) = old.lines().map(str::trim).find(|l| l.len() >= 3) else { return String::new() };
    let Some(at) = text.find(first).or_else(|| text.to_lowercase().find(&first.to_lowercase())) else {
        return "\nThe text's first line does not occur either; re-read the document with read_document.".into();
    };
    let start = text[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let mut end = (at + old.len() + 80).min(text.len());
    while !text.is_char_boundary(end) {
        end += 1;
    }
    let line = text[..start].matches('\n').count() + 1;
    format!(
        "\nNear line {line} the document actually contains (verbatim between the markers):\n-----BEGIN-----\n{}\n-----END-----\nCopy old_text exactly from this (whitespace and line breaks matter).",
        &text[start..end]
    )
}

fn pretty(v: &impl serde::Serialize) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

/// Execute a tool; returns text for the agent.
pub fn call(ctx: &mut ToolContext, name: &str, args: &Value) -> Result<String> {
    let ws = &mut *ctx.ws;
    let me = ctx.author;
    match name {
        "project_overview" => {
            let m = &ws.project.manifest;
            let main = m.paper.main.clone();
            let outline: Vec<Value> = ws
                .outline(&main)
                .unwrap_or_default()
                .into_iter()
                .map(|s| json!({ "level": s.level, "title": s.title, "label": s.label, "lines": [s.line, s.end_line] }))
                .collect();
            let open = ws.review_comments(&CommentFilter { status: Some("open".into()), ..Default::default() })?.len();
            let pending = ws.proposals(Some("pending"))?.len();
            let checkpoints: Vec<Value> = ws
                .checkpoints()?
                .into_iter()
                .take(10)
                .map(|c| json!({ "name": c.name, "kind": c.kind, "message": c.message, "at": format_ms(c.created_at) }))
                .collect();
            Ok(pretty(&json!({
                "title": m.paper.title,
                "venue": m.paper.venue,
                "format": m.paper.format,
                "main": main,
                "documents": ws.documents(),
                "outline": outline,
                "skills": { "global": m.skills.global, "sections": m.skills.sections },
                "open_review_comments": open,
                "pending_proposals": pending,
                "checkpoints": checkpoints,
                "you_are": me,
            })))
        }
        "read_document" => {
            let path = str_arg(args, "path")?;
            let (text, first_line) = match opt_str(args, "section") {
                Some(sec) => {
                    let (s, t) = ws.section(path, sec)?;
                    (t, s.line)
                }
                None => (ws.text(path)?, 1),
            };
            if args.get("line_numbers").and_then(Value::as_bool).unwrap_or(false) {
                Ok(text
                    .lines()
                    .enumerate()
                    .map(|(i, l)| format!("{:>5}  {l}", i + first_line))
                    .collect::<Vec<_>>()
                    .join("\n"))
            } else {
                Ok(text)
            }
        }
        "outline" => {
            let path = opt_str(args, "path").map(str::to_string).unwrap_or_else(|| ws.project.manifest.paper.main.clone());
            let lines: Vec<String> = ws
                .outline(&path)?
                .into_iter()
                .map(|s| {
                    format!(
                        "{}{} (lines {}-{}){}",
                        "  ".repeat(s.level.saturating_sub(1) as usize),
                        s.title,
                        s.line,
                        s.end_line,
                        s.label.map(|l| format!(" [{l}]")).unwrap_or_default()
                    )
                })
                .collect();
            Ok(if lines.is_empty() { format!("{path}: no sections found") } else { lines.join("\n") })
        }
        "search" => {
            let q = str_arg(args, "query")?.to_lowercase();
            let max = opt_int(args, "max_results").unwrap_or(50) as usize;
            let mut hits = vec![];
            'outer: for doc in ws.documents() {
                for (i, line) in ws.text(&doc)?.lines().enumerate() {
                    if line.to_lowercase().contains(&q) {
                        hits.push(format!("{doc}:{}: {}", i + 1, line.trim()));
                        if hits.len() >= max {
                            break 'outer;
                        }
                    }
                }
            }
            Ok(if hits.is_empty() { "No matches.".into() } else { hits.join("\n") })
        }
        "section_context" => {
            let ctx = ws.section_context(opt_str(args, "path"), Some(str_arg(args, "section")?))?;
            let mut out = String::new();
            if let Some(s) = &ctx.section {
                out.push_str(&format!("# {} — {} (lines {}-{})\n\n", ctx.path, s.title, s.line, s.end_line));
            }
            out.push_str("## Current text (verbatim between the markers; copy old_text from here)\n");
            out.push_str("-----BEGIN SECTION-----\n");
            out.push_str(&ctx.text);
            if !ctx.text.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("-----END SECTION-----\n\n## Skills to follow\n");
            for sk in &ctx.skills {
                out.push_str(&format!("\n### Skill: {} ({})\n\n{}\n", sk.name, sk.reason, sk.instructions));
            }
            if !ctx.open_comments.is_empty() {
                out.push_str("\n## Open review comments on this section\n");
                for c in &ctx.open_comments {
                    out.push_str(&format!("\n- #{} [{} · {}] on \"{}\": {}\n", c.id, c.severity, c.category, c.quote, c.body));
                }
            }
            if !ctx.pending_proposals.is_empty() {
                out.push_str("\n## Pending suggested edits here (do not duplicate)\n");
                for p in &ctx.pending_proposals {
                    out.push_str(&format!("\n- #{} by {}: {}\n", p.id, p.author.name, p.rationale));
                }
            }
            Ok(out)
        }
        "edit_document" => {
            let path = str_arg(args, "path")?;
            let occurrence = opt_int(args, "occurrence").map(|n| n as usize);
            let old = str_arg(args, "old_text")?;
            let out = match ws.edit(path, old, str_arg(args, "new_text")?, occurrence, me, opt_str(args, "rationale")) {
                Ok(o) => o,
                Err(e @ paper_core::Error::TextNotFound { .. }) => bail!("{e}{}", near_miss(&ws.text(path)?, old)),
                Err(e) => return Err(e.into()),
            };
            Ok(match out.update_id {
                Some(id) => format!("Edited {path} at line {} (history #{id}).", out.line),
                None => "No change (new_text equals old_text).".into(),
            })
        }
        "write_document" => {
            let path = str_arg(args, "path")?;
            let content = str_arg(args, "content")?;
            let rationale = opt_str(args, "rationale");
            let exists = ws.documents().iter().any(|d| d == path.trim_start_matches("./"));
            let id = if exists {
                ws.set_text(path, content, me, rationale)?
            } else {
                ws.create_document(path, content, me, rationale)?
            };
            Ok(format!("{} {path}{}", if exists { "Updated" } else { "Created" }, id.map(|i| format!(" (history #{i})")).unwrap_or_default()))
        }
        "propose_edit" => {
            let p = ws.propose(str_arg(args, "path")?, str_arg(args, "old_text")?, str_arg(args, "new_text")?, str_arg(args, "rationale")?, me)?;
            Ok(format!("Proposal #{} recorded for {}; a co-author will accept or reject it.", p.id, p.doc))
        }
        "list_proposals" => Ok(pretty(&ws.proposals(opt_str(args, "status"))?)),
        "history" => {
            let limit = opt_int(args, "limit").unwrap_or(20) as usize;
            let entries = ws.log(opt_str(args, "path"), limit)?;
            if entries.is_empty() {
                return Ok("No history yet.".into());
            }
            Ok(entries
                .iter()
                .map(|e| {
                    format!(
                        "#{}-{}  {}  {:<28} {:<20} +{} -{} chars{}",
                        e.first_id,
                        e.last_id,
                        format_ms(e.ended_at),
                        format!("{} ({})", e.author.name, e.author.kind.as_str()),
                        e.doc,
                        e.added,
                        e.removed,
                        if e.messages.is_empty() { String::new() } else { format!("  — {}", e.messages.join("; ")) }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "diff" => {
            let has_checkpoint = !ws.checkpoints()?.is_empty();
            let from = VersionRef::parse(opt_str(args, "from").unwrap_or(if has_checkpoint { "last" } else { "start" }));
            let to = VersionRef::parse(opt_str(args, "to").unwrap_or("now"));
            Ok(render_diffs(&ws.diff(&from, &to, opt_str(args, "path"))?))
        }
        "blame" => {
            let path = str_arg(args, "path")?;
            let authors = ws.authors()?;
            let start = opt_int(args, "start_line").unwrap_or(1).max(1) as usize;
            let end = opt_int(args, "end_line").map(|n| n as usize).unwrap_or(usize::MAX);
            Ok(ws
                .blame(path)?
                .into_iter()
                .filter(|l| l.line >= start && l.line <= end)
                .map(|l| {
                    let who = l
                        .author
                        .as_ref()
                        .map(|id| authors.get(id).map(|a| format!("{} ({})", a.name, a.kind.as_str())).unwrap_or_else(|| id.clone()))
                        .unwrap_or_else(|| "-".into());
                    format!("{:>5} {:<26} {}", l.line, who, l.text)
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "checkpoint" => {
            let kind = opt_str(args, "milestone").unwrap_or("checkpoint");
            let cp = ws.checkpoint(str_arg(args, "name")?, str_arg(args, "message")?, kind, me)?;
            Ok(format!(
                "Checkpoint '{}' saved at history #{}{}.",
                cp.name,
                cp.update_id,
                cp.git_commit.map(|c| format!(", git commit {}", &c[..c.len().min(10)])).unwrap_or_default()
            ))
        }
        "compile" => {
            let main = opt_str(args, "main").map(str::to_string).unwrap_or_else(|| ws.project.manifest.paper.main.clone());
            let mut sources = HashMap::new();
            for d in ws.documents() {
                sources.insert(d.clone(), ws.text(&d)?);
            }
            if !main.ends_with(".typ") {
                ws.wait_for_disk()?;
            }
            let out = ctx.compiler.compile(&ws.project.root, &main, &sources, CompileOptions { svg: false, pdf: true });
            let mut text = format!("{} with {} in {} ms", if out.ok { "Compiled" } else { "FAILED" }, out.engine, out.millis);
            if out.page_count > 0 {
                text.push_str(&format!(", {} pages", out.page_count));
            }
            if let Some(p) = &out.pdf_path {
                text.push_str(&format!(" → {}", p.display()));
            }
            for d in out.diagnostics.iter().take(40) {
                text.push_str(&format!(
                    "\n{} {}:{}: {}",
                    d.severity,
                    d.file.as_deref().unwrap_or("?"),
                    d.line.map(|l| l.to_string()).unwrap_or_else(|| "?".into()),
                    d.message
                ));
            }
            Ok(text)
        }
        "list_skills" => {
            let cfg = &ws.project.manifest.skills;
            let list: Vec<Value> = ws
                .skills
                .skills
                .values()
                .map(|s| {
                    json!({
                        "name": s.name, "kind": s.kind, "source": s.source, "description": s.description,
                        "auto_sections": s.sections,
                        "global": cfg.global.contains(&s.name),
                        "bound_sections": cfg.sections.iter().filter(|(_, v)| v.contains(&s.name)).map(|(k, _)| k.clone()).collect::<Vec<_>>(),
                    })
                })
                .collect();
            Ok(pretty(&list))
        }
        "get_skill" => {
            let name = str_arg(args, "name")?;
            let s = ws.skills.get(name).ok_or_else(|| anyhow!("unknown skill {name:?}; see list_skills"))?;
            Ok(format!("# Skill: {} ({})\n{}\n\n{}", s.name, s.kind, s.description, s.body))
        }
        "bind_skill" => {
            let skill = str_arg(args, "skill")?;
            if ws.skills.get(skill).is_none() {
                bail!("unknown skill {skill:?}; create it first with create_skill");
            }
            let section = opt_str(args, "section");
            ws.bind_skill(section, skill)?;
            Ok(format!("Bound {skill} to {}.", section.map(|s| format!("sections matching {s:?}")).unwrap_or_else(|| "the whole paper".into())))
        }
        "create_skill" => {
            let sections: Vec<String> = args
                .get("sections")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default();
            let path = paper_core::skills::create_skill(
                &ws.project,
                str_arg(args, "name")?,
                str_arg(args, "description")?,
                opt_str(args, "kind").unwrap_or("writing"),
                &sections,
                str_arg(args, "body")?,
            )?;
            ws.reload_skills();
            Ok(format!("Created {}", path.display()))
        }
        "start_review" => {
            let r = ws.start_review(str_arg(args, "title")?, opt_str(args, "persona"), opt_str(args, "venue"), me)?;
            let guide = ws.skills.get("peer-review").map(|s| s.body.clone()).unwrap_or_default();
            Ok(format!("Review round {} started (reviewing history #{}). round_id = {}\n\n{}", r.title, r.update_id, r.id, guide))
        }
        "add_review_comment" => {
            let round = opt_int(args, "round_id").context("missing integer argument `round_id`")?;
            let c = ws.add_review_comment(
                round,
                str_arg(args, "path")?,
                str_arg(args, "quote")?,
                str_arg(args, "severity")?,
                str_arg(args, "category")?,
                str_arg(args, "comment")?,
                opt_str(args, "section"),
            )?;
            Ok(format!("Comment #{} added ({} · {}) in {} at line {}.", c.id, c.severity, c.category, c.section.unwrap_or_else(|| "?".into()), c.current_line.unwrap_or(0)))
        }
        "finish_review" => {
            let round = opt_int(args, "round_id").context("missing integer argument `round_id`")?;
            let scores: Option<Scores> = args.get("scores").map(|v| serde_json::from_value(v.clone())).transpose()?;
            let r = ws.finish_review(round, str_arg(args, "summary")?, scores.as_ref())?;
            Ok(format!("Review round #{} finished.", r.id))
        }
        "list_review_comments" => {
            let filter = CommentFilter {
                round_id: opt_int(args, "round_id"),
                status: opt_str(args, "status").map(str::to_string),
                doc: opt_str(args, "path").map(str::to_string),
            };
            Ok(pretty(&ws.review_comments(&filter)?))
        }
        "respond_to_comment" => {
            let id = opt_int(args, "comment_id").context("missing integer argument `comment_id`")?;
            let c = ws.respond_to_comment(id, str_arg(args, "response")?, str_arg(args, "status")?, me)?;
            Ok(format!("Comment #{} marked {}.", c.id, c.status))
        }
        other => bail!("unknown tool {other:?}"),
    }
}
