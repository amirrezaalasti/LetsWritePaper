// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! LetsWritePaper desktop app: Tauri commands over a shared `Workspace`.
//!
//! The editor keeps a Yjs replica per open document and exchanges binary
//! updates with the Rust replica (base64 over IPC). A background thread tails
//! the history log (agents writing through MCP), watches the project folder
//! (other editors), and forwards everything to the UI as events.
//!
//! The same binary doubles as the MCP server (`letswritepaper mcp`) and the
//! Claude Code attribution hook (`letswritepaper ingest --from-hook`).

mod assistants;
mod claude;
mod setup;

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use paper_compile::{CompileOptions, Compiler, Diagnostic};
use paper_core::history::{Change, DocDiff, LogEntry, VersionRef};
use paper_core::outline::Section;
use paper_core::review::{ReviewComment, ReviewRound, Scores, CATEGORIES, PERSONAS, SEVERITIES};
use paper_core::skills::{AppliedSkill, SkillSource};
use paper_core::store::{Checkpoint, CommentFilter, Proposal};
use paper_core::workspace::{BlameLine, EditOutcome};
use paper_core::{Author, Format, Ownership, Project, Workspace, WorkspaceEvent};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    format!("{e:#}")
}

struct Session {
    ws: Workspace,
    _watcher: Option<RecommendedWatcher>,
    fs_events: Receiver<PathBuf>,
    pending: HashMap<String, Instant>,
}

#[derive(Default)]
struct AppState {
    session: Arc<Mutex<Option<Session>>>,
    compiler: Arc<Mutex<Compiler>>,
    /// Bumped whenever a project is opened so stale sync threads exit.
    generation: Arc<AtomicU64>,
    claude: Arc<claude::ClaudeChats>,
    /// Cross-device sync for the open project, when it is shared.
    remote: Mutex<Option<RemoteState>>,
}

struct RemoteState {
    handle: paper_sync::SyncHandle,
    /// Set when this machine hosts the relay ("share on this network").
    _relay: Option<Arc<paper_sync::Relay>>,
    /// Link other devices use to join.
    link: String,
}

impl AppState {
    fn with<T>(&self, f: impl FnOnce(&mut Workspace) -> paper_core::Result<T>) -> CmdResult<T> {
        let mut guard = self.session.lock().map_err(err)?;
        let session = guard.as_mut().ok_or("no project is open")?;
        let out = f(&mut session.ws).map_err(err)?;
        Ok(out)
    }
}

fn me() -> Author {
    Author::local_user()
}

// ---- payloads ---------------------------------------------------------------

#[derive(Serialize)]
struct ProjectInfo {
    root: String,
    title: String,
    format: Format,
    main: String,
    venue: Option<String>,
    documents: Vec<String>,
    owner: bool,
    me: Author,
    severities: Vec<&'static str>,
    categories: Vec<&'static str>,
    personas: Vec<(&'static str, &'static str)>,
}

fn project_info(ws: &Workspace) -> ProjectInfo {
    let m = &ws.project.manifest.paper;
    ProjectInfo {
        root: ws.project.root.display().to_string(),
        title: m.title.clone(),
        format: m.format,
        main: m.main.clone(),
        venue: m.venue.clone(),
        documents: ws.documents(),
        owner: ws.is_owner(),
        me: me(),
        severities: SEVERITIES.to_vec(),
        categories: CATEGORIES.to_vec(),
        personas: PERSONAS.to_vec(),
    }
}

#[derive(Clone, Serialize)]
struct DocUpdatePayload {
    path: String,
    update: String,
    author: Author,
}

#[derive(Serialize)]
struct SkillInfo {
    name: String,
    description: String,
    kind: String,
    source: SkillSource,
    auto_sections: Vec<String>,
    global: bool,
    bound_sections: Vec<String>,
    disabled: bool,
}

#[derive(Serialize)]
struct CompileResult {
    ok: bool,
    engine: String,
    millis: u128,
    diagnostics: Vec<Diagnostic>,
    page_count: usize,
    /// Typst: one SVG per page.
    svg_pages: Vec<String>,
    /// LaTeX: base64 PDF for pdf.js.
    pdf: Option<String>,
    pdf_path: Option<String>,
}

#[derive(Deserialize)]
struct CommentQuery {
    round_id: Option<i64>,
    status: Option<String>,
    path: Option<String>,
}

// ---- project lifecycle --------------------------------------------------------

fn start_session(app: &AppHandle, state: &AppState, project: Project) -> CmdResult<ProjectInfo> {
    let mut ws = Workspace::open(project, Ownership::Auto).map_err(err)?;
    ws.set_session(format!("app:{}", std::process::id()));
    ws.drain_events();
    let (tx, rx) = channel();
    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            for p in ev.paths {
                let _ = tx.send(p);
            }
        }
    })
    .ok()
    .and_then(|mut w| {
        w.watch(&ws.project.root, RecursiveMode::Recursive)
            .ok()
            .map(|_| w)
    });
    let info = project_info(&ws);
    let root = ws.project.root.clone();
    state.claude.stop_all();
    *state.remote.lock().map_err(err)? = None;
    *state.session.lock().map_err(err)? = Some(Session {
        ws,
        _watcher: watcher,
        fs_events: rx,
        pending: HashMap::new(),
    });
    if let Err(e) = start_remote(state, &root) {
        eprintln!("sync not started: {e}");
    }
    let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    spawn_sync(
        app.clone(),
        state.session.clone(),
        state.generation.clone(),
        generation,
    );
    Ok(info)
}

/// Tail other processes' edits, adopt on-disk edits, forward events to the UI.
fn spawn_sync(
    app: AppHandle,
    session: Arc<Mutex<Option<Session>>>,
    gen: Arc<AtomicU64>,
    mine: u64,
) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(200));
        if gen.load(Ordering::SeqCst) != mine {
            return;
        }
        let events = {
            let Ok(mut guard) = session.lock() else {
                return;
            };
            let Some(s) = guard.as_mut() else { return };
            let _ = s.ws.try_take_ownership();
            let _ = s.ws.poll();
            while let Ok(p) = s.fs_events.try_recv() {
                if let Some(rel) = s.ws.project.relative(&p) {
                    if s.ws.project.is_tracked_path(&rel) {
                        s.pending.insert(rel.clone(), Instant::now());
                    }
                    if rel == "paper.toml" || rel.starts_with(".paper/skills") {
                        let _ = s.ws.reload_manifest();
                    }
                }
            }
            // Debounce so agent hooks can claim the edit before it is filed as "external".
            let ready: Vec<String> = s
                .pending
                .iter()
                .filter(|(_, t)| t.elapsed() > Duration::from_millis(700))
                .map(|(p, _)| p.clone())
                .collect();
            for rel in ready {
                s.pending.remove(&rel);
                let _ = s.ws.check_disk(&rel);
            }
            s.ws.drain_events()
        };
        let mut meta = false;
        for e in events {
            match e {
                WorkspaceEvent::DocUpdate {
                    path,
                    update,
                    author,
                } => {
                    let _ = app.emit(
                        "paper://doc-update",
                        DocUpdatePayload {
                            path,
                            update: B64.encode(update),
                            author,
                        },
                    );
                }
                WorkspaceEvent::DocAdded { path } => {
                    let _ = app.emit("paper://doc-added", path);
                }
                WorkspaceEvent::DocRemoved { path } => {
                    let _ = app.emit("paper://doc-removed", path);
                }
                WorkspaceEvent::MetaChanged => meta = true,
            }
        }
        if meta {
            let _ = app.emit("paper://meta-changed", ());
        }
    });
}

#[tauri::command(rename_all = "snake_case")]
fn open_project(app: AppHandle, state: State<AppState>, path: String) -> CmdResult<ProjectInfo> {
    let project = Project::discover(Path::new(&path)).map_err(err)?;
    start_session(&app, &state, project)
}

#[tauri::command(rename_all = "snake_case")]
fn init_project(
    app: AppHandle,
    state: State<AppState>,
    path: String,
    title: String,
    format: Format,
) -> CmdResult<ProjectInfo> {
    let project = Project::init(Path::new(&path), &title, format).map_err(err)?;
    let info = start_session(&app, &state, project.clone())?;
    if let Ok(exe) = std::env::current_exe() {
        let _ = paper_core::agents::write_agent_config(
            &project,
            &paper_core::agents::Launcher { exe: &exe },
        );
    }
    Ok(info)
}

#[tauri::command]
fn project_info_cmd(state: State<AppState>) -> CmdResult<ProjectInfo> {
    state.with(|ws| Ok(project_info(ws)))
}

/// Write .mcp.json, the Claude Code hook and AGENTS.md pointing at this app binary.
#[tauri::command]
fn setup_agents(state: State<AppState>) -> CmdResult<Vec<String>> {
    let exe = std::env::current_exe().map_err(err)?;
    state.with(|ws| {
        paper_core::agents::write_agent_config(
            &ws.project,
            &paper_core::agents::Launcher { exe: &exe },
        )
    })
}

#[tauri::command]
fn agent_command() -> CmdResult<String> {
    let exe = std::env::current_exe().map_err(err)?;
    Ok(format!("claude mcp add paper -- \"{}\" mcp", exe.display()))
}

// ---- documents -------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
fn doc_state(state: State<AppState>, path: String) -> CmdResult<String> {
    state.with(|ws| ws.doc_state(&path)).map(|b| B64.encode(b))
}

#[tauri::command(rename_all = "snake_case")]
fn doc_text(state: State<AppState>, path: String) -> CmdResult<String> {
    state.with(|ws| ws.text(&path))
}

/// A Yjs update produced by the editor.
#[tauri::command(rename_all = "snake_case")]
fn push_update(state: State<AppState>, path: String, update: String) -> CmdResult<i64> {
    let bytes = B64.decode(update).map_err(err)?;
    state.with(|ws| ws.apply_update(&path, &bytes, &me(), None))
}

#[tauri::command(rename_all = "snake_case")]
fn create_document(state: State<AppState>, path: String, content: String) -> CmdResult<()> {
    state.with(|ws| {
        ws.create_document(&path, &content, &me(), Some("new document"))
            .map(|_| ())
    })
}

#[tauri::command(rename_all = "snake_case")]
fn delete_document(state: State<AppState>, path: String) -> CmdResult<()> {
    state.with(|ws| ws.delete_document(&path))
}

#[tauri::command(rename_all = "snake_case")]
fn outline(state: State<AppState>, path: String) -> CmdResult<Vec<Section>> {
    state.with(|ws| ws.outline(&path))
}

// ---- history ---------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
fn history(
    state: State<AppState>,
    path: Option<String>,
    limit: Option<usize>,
) -> CmdResult<Vec<LogEntry>> {
    state.with(|ws| ws.log(path.as_deref(), limit.unwrap_or(100)))
}

/// Every edit, newest first, each with a description of what changed.
#[tauri::command(rename_all = "snake_case")]
fn changes(
    state: State<AppState>,
    path: Option<String>,
    author: Option<String>,
    limit: Option<usize>,
) -> CmdResult<Vec<Change>> {
    state.with(|ws| ws.changes(path.as_deref(), author.as_deref(), limit.unwrap_or(200)))
}

#[tauri::command(rename_all = "snake_case")]
fn diff(
    state: State<AppState>,
    from: String,
    to: String,
    path: Option<String>,
) -> CmdResult<Vec<DocDiff>> {
    state.with(|ws| {
        ws.diff(
            &VersionRef::parse(&from),
            &VersionRef::parse(&to),
            path.as_deref(),
        )
    })
}

#[tauri::command(rename_all = "snake_case")]
fn text_at(state: State<AppState>, path: String, version: String) -> CmdResult<Option<String>> {
    state.with(|ws| {
        let resolved = ws.resolve_version(&VersionRef::parse(&version))?;
        ws.text_at(&path, &resolved)
    })
}

#[derive(Serialize)]
struct BlameResult {
    lines: Vec<BlameLine>,
    authors: HashMap<String, Author>,
}

#[tauri::command(rename_all = "snake_case")]
fn blame(state: State<AppState>, path: String) -> CmdResult<BlameResult> {
    state.with(|ws| {
        Ok(BlameResult {
            lines: ws.blame(&path)?,
            authors: ws.authors()?,
        })
    })
}

#[tauri::command(rename_all = "snake_case")]
fn checkpoints(state: State<AppState>) -> CmdResult<Vec<Checkpoint>> {
    state.with(|ws| ws.checkpoints())
}

#[tauri::command(rename_all = "snake_case")]
fn create_checkpoint(
    state: State<AppState>,
    name: String,
    message: String,
    kind: Option<String>,
) -> CmdResult<Checkpoint> {
    state.with(|ws| {
        ws.checkpoint(
            &name,
            &message,
            kind.as_deref().unwrap_or("checkpoint"),
            &me(),
        )
    })
}

#[tauri::command(rename_all = "snake_case")]
fn restore(state: State<AppState>, path: String, version: String) -> CmdResult<Option<i64>> {
    state.with(|ws| ws.restore(&path, &VersionRef::parse(&version), &me()))
}

// ---- proposals ---------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
fn proposals(state: State<AppState>, status: Option<String>) -> CmdResult<Vec<Proposal>> {
    state.with(|ws| ws.proposals(status.as_deref()))
}

#[tauri::command(rename_all = "snake_case")]
fn propose_edit(
    state: State<AppState>,
    path: String,
    old_text: String,
    new_text: String,
    rationale: String,
) -> CmdResult<Proposal> {
    state.with(|ws| ws.propose(&path, &old_text, &new_text, &rationale, &me()))
}

#[tauri::command(rename_all = "snake_case")]
fn accept_proposal(state: State<AppState>, id: i64) -> CmdResult<EditOutcome> {
    state.with(|ws| ws.accept_proposal(id, &me()))
}

#[tauri::command(rename_all = "snake_case")]
fn reject_proposal(state: State<AppState>, id: i64) -> CmdResult<()> {
    state.with(|ws| ws.reject_proposal(id, &me()))
}

// ---- peer review ---------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
fn review_rounds(state: State<AppState>) -> CmdResult<Vec<ReviewRound>> {
    state.with(|ws| ws.review_rounds())
}

#[tauri::command(rename_all = "snake_case")]
fn review_comments(
    state: State<AppState>,
    query: Option<CommentQuery>,
) -> CmdResult<Vec<ReviewComment>> {
    let q = query.unwrap_or(CommentQuery {
        round_id: None,
        status: None,
        path: None,
    });
    state.with(|ws| {
        ws.review_comments(&CommentFilter {
            round_id: q.round_id,
            status: q.status,
            doc: q.path,
        })
    })
}

/// Humans can review too (e.g. an advisor's internal review round).
#[tauri::command(rename_all = "snake_case")]
fn start_review(
    state: State<AppState>,
    title: String,
    persona: Option<String>,
    venue: Option<String>,
) -> CmdResult<ReviewRound> {
    state.with(|ws| ws.start_review(&title, persona.as_deref(), venue.as_deref(), &me()))
}

#[tauri::command(rename_all = "snake_case")]
fn add_review_comment(
    state: State<AppState>,
    round_id: i64,
    path: String,
    quote: String,
    severity: String,
    category: String,
    comment: String,
) -> CmdResult<ReviewComment> {
    state.with(|ws| {
        ws.add_review_comment(
            round_id, &path, &quote, &severity, &category, &comment, None,
        )
    })
}

#[tauri::command(rename_all = "snake_case")]
fn finish_review(
    state: State<AppState>,
    round_id: i64,
    summary: String,
    scores: Option<Scores>,
) -> CmdResult<ReviewRound> {
    state.with(|ws| ws.finish_review(round_id, &summary, scores.as_ref()))
}

#[tauri::command(rename_all = "snake_case")]
fn respond_to_comment(
    state: State<AppState>,
    id: i64,
    response: String,
    status: String,
) -> CmdResult<ReviewComment> {
    state.with(|ws| ws.respond_to_comment(id, &response, &status, &me()))
}

#[tauri::command(rename_all = "snake_case")]
fn response_letter(state: State<AppState>) -> CmdResult<String> {
    state.with(|ws| ws.response_letter())
}

// ---- skills ------------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
fn skills(state: State<AppState>) -> CmdResult<Vec<SkillInfo>> {
    state.with(|ws| {
        let cfg = ws.project.manifest.skills.clone();
        Ok(ws
            .skills
            .skills
            .values()
            .map(|s| SkillInfo {
                name: s.name.clone(),
                description: s.description.clone(),
                kind: s.kind.clone(),
                source: s.source,
                auto_sections: s.sections.clone(),
                global: cfg.global.contains(&s.name),
                bound_sections: cfg
                    .sections
                    .iter()
                    .filter(|(_, v)| v.contains(&s.name))
                    .map(|(k, _)| k.clone())
                    .collect(),
                disabled: cfg.disabled.contains(&s.name),
            })
            .collect())
    })
}

#[tauri::command(rename_all = "snake_case")]
fn skill_body(state: State<AppState>, name: String) -> CmdResult<String> {
    state.with(|ws| {
        ws.skills
            .get(&name)
            .map(|s| s.body.clone())
            .ok_or_else(|| paper_core::Error::NotFound(format!("skill {name}")))
    })
}

#[tauri::command(rename_all = "snake_case")]
fn skills_for_section(state: State<AppState>, section: String) -> CmdResult<Vec<AppliedSkill>> {
    state.with(|ws| Ok(ws.skills_for(Some(&section))))
}

#[tauri::command(rename_all = "snake_case")]
fn bind_skill(state: State<AppState>, skill: String, section: Option<String>) -> CmdResult<()> {
    state.with(|ws| ws.bind_skill(section.as_deref(), &skill))
}

#[tauri::command(rename_all = "snake_case")]
fn unbind_skill(state: State<AppState>, skill: String, section: Option<String>) -> CmdResult<()> {
    state.with(|ws| ws.unbind_skill(section.as_deref(), &skill))
}

/// Install a skill folder (containing SKILL.md) into the project or, with `user`, for all projects.
#[tauri::command(rename_all = "snake_case")]
fn install_skill(state: State<AppState>, path: String, user: bool) -> CmdResult<String> {
    state.with(|ws| {
        let name = paper_core::skills::install_skill(
            Path::new(&path),
            if user { None } else { Some(&ws.project) },
        )?;
        ws.reload_skills();
        Ok(name)
    })
}

#[tauri::command(rename_all = "snake_case")]
fn create_skill(
    state: State<AppState>,
    name: String,
    description: String,
    kind: String,
    sections: Vec<String>,
    body: String,
) -> CmdResult<String> {
    state.with(|ws| {
        let p = paper_core::skills::create_skill(
            &ws.project,
            &name,
            &description,
            &kind,
            &sections,
            &body,
        )?;
        ws.reload_skills();
        Ok(p.display().to_string())
    })
}

// ---- compile -----------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
async fn compile(state: State<'_, AppState>, main: Option<String>) -> CmdResult<CompileResult> {
    let (root, main, sources) = {
        let guard = state.session.lock().map_err(err)?;
        let s = guard.as_ref().ok_or("no project is open")?;
        let main = main.unwrap_or_else(|| s.ws.project.manifest.paper.main.clone());
        let mut sources = HashMap::new();
        for d in s.ws.documents() {
            sources.insert(d.clone(), s.ws.text(&d).map_err(err)?);
        }
        (s.ws.project.root.clone(), main, sources)
    };
    let compiler = state.compiler.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut c = compiler.lock().map_err(err)?;
        let out = c.compile(
            &root,
            &main,
            &sources,
            CompileOptions {
                svg: true,
                pdf: false,
            },
        );
        let pdf = if main.ends_with(".typ") {
            None
        } else {
            out.pdf_path
                .as_ref()
                .and_then(|p| std::fs::read(p).ok())
                .map(|b| B64.encode(b))
        };
        Ok(CompileResult {
            ok: out.ok,
            engine: out.engine,
            millis: out.millis,
            diagnostics: out.diagnostics,
            page_count: out.page_count,
            svg_pages: out.svg_pages,
            pdf,
            pdf_path: out.pdf_path.map(|p| p.display().to_string()),
        })
    })
    .await
    .map_err(err)?
}

/// Compile the paper and write the PDF to `dest` (chosen in the save dialog).
#[tauri::command(rename_all = "snake_case")]
async fn export_pdf(state: State<'_, AppState>, dest: String) -> CmdResult<String> {
    let dest = pdf_dest(&dest).ok_or("choose where to save the PDF")?;
    let (root, main, sources) = {
        let guard = state.session.lock().map_err(err)?;
        let s = guard.as_ref().ok_or("no project is open")?;
        let mut sources = HashMap::new();
        for d in s.ws.documents() {
            sources.insert(d.clone(), s.ws.text(&d).map_err(err)?);
        }
        (
            s.ws.project.root.clone(),
            s.ws.project.manifest.paper.main.clone(),
            sources,
        )
    };
    let compiler = state.compiler.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let out = compiler.lock().map_err(err)?.compile(
            &root,
            &main,
            &sources,
            CompileOptions {
                svg: false,
                pdf: true,
            },
        );
        let src = out.pdf_path.clone().ok_or_else(|| compile_failure(&out))?;
        if let Some(parent) = dest.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(err)?;
            }
        }
        std::fs::copy(&src, &dest).map_err(err)?;
        Ok(dest.display().to_string())
    })
    .await
    .map_err(err)?
}

fn pdf_dest(dest: &str) -> Option<std::path::PathBuf> {
    let dest = dest.trim();
    if dest.is_empty() {
        return None;
    }
    let path = std::path::PathBuf::from(dest);
    let pdf = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"));
    Some(if pdf { path } else { path.with_extension("pdf") })
}

fn compile_failure(out: &paper_compile::CompileOutput) -> String {
    let errors: Vec<String> = out
        .diagnostics
        .iter()
        .filter(|d| d.severity == "error")
        .take(3)
        .map(|d| match (&d.file, d.line) {
            (Some(file), Some(line)) => format!("{file}:{line}: {}", d.message),
            (Some(file), None) => format!("{file}: {}", d.message),
            (None, Some(line)) => format!("line {line}: {}", d.message),
            (None, None) => d.message.clone(),
        })
        .collect();
    if errors.is_empty() {
        "compilation produced no PDF".into()
    } else {
        errors.join("\n")
    }
}

// ---- sharing across devices -------------------------------------------------------

/// Resume syncing (and hosting, if this machine hosts the relay) for a shared project.
fn start_remote(state: &AppState, root: &Path) -> Result<(), String> {
    let Some(cfg) = paper_sync::RemoteConfig::load(root) else {
        return Ok(());
    };
    let (relay, link) = match paper_sync::resume_hosting(&cfg) {
        Ok(Some((relay, link))) => (Some(relay), link),
        Ok(None) => (None, cfg.link()),
        // The port is taken: this app instance is already hosting it.
        Err(_) => (None, cfg.public_link()),
    };
    let handle = paper_sync::start(root)
        .map_err(err)?
        .ok_or("project is not shared")?;
    *state.remote.lock().map_err(err)? = Some(RemoteState {
        handle,
        _relay: relay,
        link,
    });
    Ok(())
}

#[derive(Serialize)]
struct ShareInfo {
    shared: bool,
    link: Option<String>,
    hosted_here: bool,
    status: Option<paper_sync::SyncStatus>,
}

#[tauri::command]
fn share_info(state: State<AppState>) -> CmdResult<ShareInfo> {
    let root = state.with(|ws| Ok(ws.project.root.clone()))?;
    let cfg = paper_sync::RemoteConfig::load(&root);
    let remote = state.remote.lock().map_err(err)?;
    Ok(ShareInfo {
        shared: cfg.is_some(),
        link: remote
            .as_ref()
            .map(|r| r.link.clone())
            .or_else(|| cfg.as_ref().map(|c| c.public_link())),
        hosted_here: cfg.as_ref().is_some_and(|c| c.hosted.is_some()),
        status: remote.as_ref().map(|r| r.handle.snapshot()),
    })
}

/// Share the open paper. `mode`: `network` hosts a relay in this app for
/// devices on the same network; `server` uses a relay at `server`.
#[tauri::command(rename_all = "snake_case")]
async fn share_project(
    app: AppHandle,
    state: State<'_, AppState>,
    mode: String,
    server: Option<String>,
    secret: Option<String>,
) -> CmdResult<ShareInfo> {
    let (project, root) = state.with(|ws| Ok((ws.project.clone(), ws.project.root.clone())))?;
    let data = app.path().app_data_dir().map_err(err)?.join("relay");
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        if mode == "network" {
            let (relay, _) =
                paper_sync::share_on_this_network(&project, &data, paper_sync::DEFAULT_PORT)
                    .or_else(|_| paper_sync::share_on_this_network(&project, &data, 0))
                    .map_err(err)?;
            drop(relay);
            Ok(())
        } else {
            let server = server
                .filter(|s| !s.trim().is_empty())
                .ok_or("enter the relay server URL")?;
            paper_sync::share(
                &project,
                server.trim(),
                secret.as_deref().filter(|s| !s.is_empty()),
            )
            .map(|_| ())
            .map_err(err)
        }
    })
    .await
    .map_err(err)?;
    result?;
    // A hosted relay keeps serving on its own thread after `share_on_this_network`.
    let link = paper_sync::RemoteConfig::load(&root)
        .ok_or("sharing failed")?
        .public_link();
    let handle = paper_sync::start(&root)
        .map_err(err)?
        .ok_or("project is not shared")?;
    *state.remote.lock().map_err(err)? = Some(RemoteState {
        handle,
        _relay: None,
        link,
    });
    share_info(state)
}

#[tauri::command]
fn stop_sharing(state: State<AppState>) -> CmdResult<()> {
    let root = state.with(|ws| Ok(ws.project.root.clone()))?;
    *state.remote.lock().map_err(err)? = None;
    paper_sync::RemoteConfig::remove(&root).map_err(err)
}

#[tauri::command(rename_all = "snake_case")]
async fn join_project(
    app: AppHandle,
    state: State<'_, AppState>,
    link: String,
    dir: String,
) -> CmdResult<ProjectInfo> {
    let dir = PathBuf::from(dir);
    let project =
        tauri::async_runtime::spawn_blocking(move || paper_sync::join(&link, &dir).map_err(err))
            .await
            .map_err(err)??;
    start_session(&app, &state, project)
}

// ---- Claude panel ----------------------------------------------------------------

#[tauri::command]
async fn setup_status() -> CmdResult<setup::SetupStatus> {
    tauri::async_runtime::spawn_blocking(setup::status)
        .await
        .map_err(err)
}

#[tauri::command(rename_all = "snake_case")]
fn setup_save(name: String, setup_done: bool) -> CmdResult<setup::SetupStatus> {
    setup::save(name, setup_done)
}

#[tauri::command]
async fn install_tectonic(app: AppHandle) -> CmdResult<setup::SetupStatus> {
    tauri::async_runtime::spawn_blocking(move || setup::install_tectonic(&app))
        .await
        .map_err(err)?
}

#[tauri::command]
async fn claude_status(assistant: Option<String>) -> CmdResult<claude::ClaudeStatus> {
    tauri::async_runtime::spawn_blocking(move || {
        claude::status_for(assistant.as_deref().unwrap_or("claude"))
    })
    .await
    .map_err(err)
}

#[tauri::command]
async fn claude_install(
    app: AppHandle,
    assistant: Option<String>,
) -> CmdResult<claude::ClaudeStatus> {
    tauri::async_runtime::spawn_blocking(move || {
        claude::install_for(&app, assistant.as_deref().unwrap_or("claude"))
    })
    .await
    .map_err(err)?
}

#[tauri::command]
fn claude_sessions(state: State<AppState>) -> CmdResult<Vec<claude::SessionInfo>> {
    let root = state.with(|ws| Ok(ws.project.root.clone()))?;
    Ok(claude::sessions(&root))
}

#[tauri::command(rename_all = "snake_case")]
#[allow(clippy::too_many_arguments)]
fn claude_start(
    app: AppHandle,
    state: State<AppState>,
    chat_id: String,
    resume: Option<String>,
    mode: String,
    model: Option<String>,
    provider: Option<String>,
    foundry_resource: Option<String>,
    foundry_api_key: Option<String>,
    assistant: Option<String>,
    api_key: Option<String>,
) -> CmdResult<()> {
    let root = state.with(|ws| Ok(ws.project.root.clone()))?;
    state.claude.start(
        &app,
        &root,
        &chat_id,
        resume.as_deref(),
        &mode,
        model.as_deref(),
        provider.as_deref(),
        foundry_resource.as_deref(),
        foundry_api_key.as_deref(),
        assistant.as_deref(),
        api_key.as_deref(),
    )
}

#[tauri::command(rename_all = "snake_case")]
fn claude_send(
    app: AppHandle,
    state: State<AppState>,
    chat_id: String,
    text: String,
    context: Option<claude::EditorContext>,
    attachments: Option<Vec<claude::ChatAttachment>>,
) -> CmdResult<()> {
    let root = state.with(|ws| Ok(ws.project.root.clone()))?;
    state.claude.send(
        &app,
        &root,
        &chat_id,
        &text,
        &context.unwrap_or_default(),
        &attachments.unwrap_or_default(),
    )
}

#[tauri::command(rename_all = "snake_case")]
fn claude_permission(
    app: AppHandle,
    state: State<AppState>,
    chat_id: String,
    reply: claude::PermissionReply,
) -> CmdResult<()> {
    state.claude.permission(&app, &chat_id, &reply)
}

#[tauri::command(rename_all = "snake_case")]
fn claude_stop(state: State<AppState>, chat_id: String) -> CmdResult<Option<String>> {
    Ok(state.claude.stop(&chat_id))
}

#[tauri::command]
async fn claude_auth(assistant: Option<String>) -> CmdResult<claude::ClaudeAuth> {
    tauri::async_runtime::spawn_blocking(move || {
        claude::auth_for(assistant.as_deref().unwrap_or("claude"))
    })
    .await
    .map_err(err)
}

#[tauri::command(rename_all = "snake_case")]
async fn assistant_store_key(assistant: String, api_key: String) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || claude::login_api_key(&assistant, &api_key))
        .await
        .map_err(err)?
}

/// Open a terminal for `login`, `resume` or `remote` (continue on another device).
/// `provider` is `claudeai`, `console`, `bedrock`, `vertex`, or `foundry`.
#[tauri::command(rename_all = "snake_case")]
fn claude_terminal(
    state: State<AppState>,
    action: String,
    session: Option<String>,
    chat_id: Option<String>,
    provider: Option<String>,
    foundry_resource: Option<String>,
    assistant: Option<String>,
) -> CmdResult<String> {
    let (root, title) = state.with(|ws| {
        Ok((
            ws.project.root.clone(),
            ws.project.manifest.paper.title.clone(),
        ))
    })?;
    // A session must not run in two processes at once: hand it over.
    if let Some(id) = chat_id {
        state.claude.stop(&id);
    }
    claude::open_in_terminal(
        &root,
        &action,
        session.as_deref(),
        &title,
        provider.as_deref(),
        foundry_resource.as_deref(),
        assistant.as_deref(),
    )
}

/// `letswritepaper mcp` / `letswritepaper ingest --from-hook` entry points.
fn run_cli_mode() -> Option<i32> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("mcp") => {
            let agent = args
                .iter()
                .position(|a| a == "--agent")
                .and_then(|i| args.get(i + 1))
                .cloned();
            let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
            Some(match paper_mcp::serve_stdio(&cwd, agent.as_deref()) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("error: {e:#}");
                    1
                }
            })
        }
        Some("ingest") if args.iter().any(|a| a == "--from-hook") => {
            let author = args
                .iter()
                .position(|a| a == "--author")
                .and_then(|i| args.get(i + 1))
                .map(|s| Author::parse(s))
                .unwrap_or_else(|| Author::agent("claude-code"));
            let mut input = String::new();
            let _ = std::io::stdin().read_to_string(&mut input);
            match paper_core::agents::ingest_from_hook(&input, &author) {
                Ok(Some(msg)) => eprintln!("{msg}"),
                Ok(None) => {}
                Err(e) => eprintln!("paper: could not record edit: {e}"),
            }
            Some(0)
        }
        _ => None,
    }
}

fn main() {
    if let Some(code) = run_cli_mode() {
        std::process::exit(code);
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(AppState::default())
        .setup(|app| {
            // `letswritepaper /path/to/paper` opens that project on launch.
            if let Some(path) = std::env::args().nth(1) {
                let handle = app.handle().clone();
                let state = app.state::<AppState>();
                if let Ok(project) = Project::discover(Path::new(&path)) {
                    let _ = start_session(&handle, &state, project);
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_project,
            init_project,
            project_info_cmd,
            setup_agents,
            agent_command,
            doc_state,
            doc_text,
            push_update,
            create_document,
            delete_document,
            outline,
            history,
            changes,
            diff,
            text_at,
            blame,
            checkpoints,
            create_checkpoint,
            restore,
            proposals,
            propose_edit,
            accept_proposal,
            reject_proposal,
            review_rounds,
            review_comments,
            start_review,
            add_review_comment,
            finish_review,
            respond_to_comment,
            response_letter,
            skills,
            skill_body,
            skills_for_section,
            bind_skill,
            unbind_skill,
            install_skill,
            create_skill,
            compile,
            export_pdf,
            share_info,
            share_project,
            stop_sharing,
            join_project,
            setup_status,
            setup_save,
            install_tectonic,
            claude_status,
            claude_auth,
            assistant_store_key,
            claude_install,
            claude_sessions,
            claude_start,
            claude_send,
            claude_permission,
            claude_stop,
            claude_terminal,
        ])
        .run(tauri::generate_context!())
        .expect("error while running LetsWritePaper");
}
