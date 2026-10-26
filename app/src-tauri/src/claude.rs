//! The Claude panel: runs the user's own Claude Code CLI as a streaming
//! subprocess (like the VS Code / Cursor extension), wired to this paper's MCP
//! server so every edit Claude makes is attributed and visible live.
//!
//! Protocol: `claude -p --input-format stream-json --output-format stream-json`
//! — one JSON message per line each way. Sessions are resumable with
//! `--resume <id>`; continuing on another device uses Claude Code's Remote
//! Control (`claude --resume <id> --remote-control`), opened in a terminal.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

const SYSTEM_PROMPT: &str = "You are working inside LetsWritePaper, a desktop editor for academic papers, as the user's co-author. \
Use the paper's MCP tools (named mcp__paper__<tool>, e.g. mcp__paper__project_overview) for everything about the paper: project_overview to orient yourself; section_context before \
changing a section (follow the skills it returns and consider open review comments); edit_document with exact old_text \
and a clear rationale for focused edits; propose_edit when the user asked for suggestions or the change is substantive; \
compile to check the paper still builds. The user sees your edits appear live in their editor and in the history under \
your name. Never invent citations: cite only keys present in the project's .bib files, otherwise write TODO. \
Keep replies short; the paper itself is where your work goes.";

#[derive(Clone, Debug, Serialize)]
pub struct ClaudeStatus {
    pub installed: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub install_command: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
}

struct Chat {
    child: Child,
    stdin: ChildStdin,
    session_id: Arc<Mutex<Option<String>>>,
}

#[derive(Default)]
pub struct ClaudeChats {
    chats: Mutex<HashMap<String, Chat>>,
}

#[derive(Clone, Serialize)]
struct ChatEvent {
    chat_id: String,
    event: Value,
}

#[derive(Deserialize, Default)]
pub struct EditorContext {
    pub path: Option<String>,
    pub line: Option<usize>,
    pub section: Option<String>,
    pub selection: Option<String>,
}

// ---- finding and installing the CLI ------------------------------------------------

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    }
}

/// Directories GUI apps often lack on PATH but installers use.
fn extra_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![];
    if let Some(home) = home_dir() {
        dirs.push(home.join(".local").join("bin"));
        dirs.push(home.join(".claude").join("local"));
        dirs.push(home.join(".npm-global").join("bin"));
        dirs.push(home.join(".bun").join("bin"));
        if cfg!(windows) {
            if let Ok(appdata) = std::env::var("APPDATA") {
                dirs.push(PathBuf::from(appdata).join("npm"));
            }
        }
    }
    if !cfg!(windows) {
        dirs.push("/opt/homebrew/bin".into());
        dirs.push("/usr/local/bin".into());
    }
    dirs
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// PATH for child processes: the inherited one plus common install locations.
fn child_path() -> std::ffi::OsString {
    let mut paths: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    for d in extra_dirs() {
        if !paths.contains(&d) {
            paths.push(d);
        }
    }
    std::env::join_paths(paths).unwrap_or_default()
}

pub fn find_claude() -> Option<PathBuf> {
    let path = child_path();
    for dir in std::env::split_paths(&path) {
        for name in [exe_name(), "claude.cmd"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    // Fall back to the user's login shell, which knows their real PATH.
    if !cfg!(windows) {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        if let Ok(out) = Command::new(shell).args(["-lc", "command -v claude"]).output() {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() && Path::new(&p).is_file() {
                return Some(PathBuf::from(p));
            }
        }
    }
    None
}

pub fn install_command() -> String {
    if cfg!(windows) {
        "irm https://claude.ai/install.ps1 | iex".into()
    } else {
        "curl -fsSL https://claude.ai/install.sh | bash".into()
    }
}

pub fn status() -> ClaudeStatus {
    let path = find_claude();
    let version = path.as_ref().and_then(|p| {
        Command::new(p)
            .arg("--version")
            .env("PATH", child_path())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    });
    ClaudeStatus {
        installed: path.is_some(),
        path: path.map(|p| p.display().to_string()),
        version,
        install_command: install_command(),
    }
}

/// Run the official installer, streaming its output to the UI.
pub fn install(app: &AppHandle) -> Result<ClaudeStatus, String> {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &install_command()]);
        c
    } else {
        let mut c = Command::new("/bin/bash");
        c.args(["-c", &install_command()]);
        c
    };
    let mut child = cmd
        .env("PATH", child_path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start the installer: {e}"))?;
    let stderr = child.stderr.take();
    let app2 = app.clone();
    let err_thread = std::thread::spawn(move || {
        if let Some(s) = stderr {
            for line in BufReader::new(s).lines().map_while(Result::ok) {
                let _ = app2.emit("claude://install-log", line);
            }
        }
    });
    if let Some(s) = child.stdout.take() {
        for line in BufReader::new(s).lines().map_while(Result::ok) {
            let _ = app.emit("claude://install-log", line);
        }
    }
    let _ = err_thread.join();
    let code = child.wait().map_err(|e| e.to_string())?;
    if !code.success() {
        return Err(format!("installer exited with {code}"));
    }
    Ok(status())
}

// ---- sessions --------------------------------------------------------------------

fn sessions_file(root: &Path) -> PathBuf {
    root.join(".paper").join("claude-sessions.json")
}

pub fn sessions(root: &Path) -> Vec<SessionInfo> {
    let mut list: Vec<SessionInfo> = std::fs::read_to_string(sessions_file(root))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    list.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
    list
}

fn touch_session(root: &Path, id: &str, title: Option<&str>) {
    let mut list = sessions(root);
    let now = paper_core::now_ms();
    match list.iter_mut().find(|s| s.id == id) {
        Some(s) => {
            s.updated_at = now;
            if s.title.is_empty() {
                if let Some(t) = title {
                    s.title = t.to_string();
                }
            }
        }
        None => list.push(SessionInfo { id: id.into(), title: title.unwrap_or("").into(), created_at: now, updated_at: now }),
    }
    if let Ok(json) = serde_json::to_string_pretty(&list) {
        let _ = std::fs::write(sessions_file(root), json);
    }
}

// ---- chats -----------------------------------------------------------------------

fn mcp_config() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(json!({ "mcpServers": { "paper": { "command": exe, "args": ["mcp", "--agent", "claude-code"] } } }).to_string())
}

fn base_args(claude: &Path, root: &Path) -> Result<Command, String> {
    let mut cmd = Command::new(claude);
    cmd.current_dir(root).env("PATH", child_path()).env("PAPER_AGENT", "claude-code");
    // Only the paper's server: faster start-up and no unrelated tools.
    cmd.args(["--mcp-config", &mcp_config()?, "--strict-mcp-config"]);
    Ok(cmd)
}

impl ClaudeChats {
    /// Start (or resume) a chat. `mode`: `paper` (paper tools + reading only)
    /// or `edits` (also lets Claude edit files and use its other tools).
    pub fn start(&self, app: &AppHandle, root: &Path, chat_id: &str, resume: Option<&str>, mode: &str, model: Option<&str>) -> Result<(), String> {
        self.stop(chat_id);
        let claude = find_claude().ok_or("Claude Code is not installed")?;
        let mut cmd = base_args(&claude, root)?;
        cmd.args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--append-system-prompt",
            SYSTEM_PROMPT,
            "--allowedTools",
            "mcp__paper__*,Read,Grep,Glob",
        ]);
        if mode == "edits" {
            cmd.args(["--permission-mode", "acceptEdits"]);
        }
        if let Some(id) = resume {
            cmd.args(["--resume", id]);
        }
        if let Some(m) = model.filter(|m| !m.is_empty()) {
            cmd.args(["--model", m]);
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start Claude Code: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take();
        let session_id = Arc::new(Mutex::new(resume.map(str::to_string)));

        let (app2, id2, root2, sid2) = (app.clone(), chat_id.to_string(), root.to_path_buf(), session_id.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Ok(event) = serde_json::from_str::<Value>(&line) else { continue };
                if event.get("type").and_then(Value::as_str) == Some("system") && event.get("subtype").and_then(Value::as_str) == Some("init") {
                    if let Some(sid) = event.get("session_id").and_then(Value::as_str) {
                        *sid2.lock().unwrap() = Some(sid.to_string());
                        touch_session(&root2, sid, None);
                    }
                }
                let _ = app2.emit("claude://event", ChatEvent { chat_id: id2.clone(), event });
            }
            let _ = app2.emit("claude://event", ChatEvent { chat_id: id2.clone(), event: json!({ "type": "exit" }) });
        });
        if let Some(stderr) = stderr {
            let (app3, id3) = (app.clone(), chat_id.to_string());
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let _ = app3.emit("claude://event", ChatEvent { chat_id: id3.clone(), event: json!({ "type": "stderr", "text": line }) });
                }
            });
        }
        self.chats.lock().unwrap().insert(chat_id.to_string(), Chat { child, stdin, session_id });
        Ok(())
    }

    pub fn send(&self, root: &Path, chat_id: &str, text: &str, ctx: &EditorContext) -> Result<(), String> {
        let mut chats = self.chats.lock().unwrap();
        let chat = chats.get_mut(chat_id).ok_or("this chat is not running")?;
        let mut content = String::new();
        if ctx.path.is_some() || ctx.selection.is_some() {
            content.push_str("<editor_context>\n");
            if let Some(p) = &ctx.path {
                content.push_str(&format!("The user is viewing {p}"));
                if let Some(l) = ctx.line {
                    content.push_str(&format!(", line {l}"));
                }
                if let Some(s) = &ctx.section {
                    content.push_str(&format!(", in the section \"{s}\""));
                }
                content.push_str(".\n");
            }
            if let Some(sel) = ctx.selection.as_ref().filter(|s| !s.trim().is_empty()) {
                content.push_str(&format!("Selected text:\n```\n{sel}\n```\n"));
            }
            content.push_str("</editor_context>\n\n");
        }
        content.push_str(text);
        let line = json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": content }] },
            "parent_tool_use_id": null,
        });
        writeln!(chat.stdin, "{line}").map_err(|e| format!("Claude Code is not accepting input: {e}"))?;
        chat.stdin.flush().map_err(|e| e.to_string())?;
        if let Some(sid) = chat.session_id.lock().unwrap().clone() {
            let title: String = text.chars().take(60).collect();
            touch_session(root, &sid, Some(&title));
        }
        Ok(())
    }

    pub fn stop(&self, chat_id: &str) -> Option<String> {
        let mut chats = self.chats.lock().unwrap();
        let mut chat = chats.remove(chat_id)?;
        let _ = chat.child.kill();
        let _ = chat.child.wait();
        let sid = chat.session_id.lock().unwrap().clone();
        sid
    }

    pub fn stop_all(&self) {
        let ids: Vec<String> = self.chats.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.stop(&id);
        }
    }
}

// ---- terminals (login, Remote Control) ------------------------------------------------

fn shell_quote(s: &str) -> String {
    if cfg!(windows) {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Open the platform terminal in `dir` running `command`.
fn open_terminal(dir: &Path, command: &str) -> Result<(), String> {
    let dir_s = dir.display().to_string();
    if cfg!(target_os = "macos") {
        let script = format!("cd {} && {}", shell_quote(&dir_s), command);
        let escaped = script.replace('\\', "\\\\").replace('"', "\\\"");
        Command::new("osascript")
            .args(["-e", &format!("tell application \"Terminal\" to do script \"{escaped}\""), "-e", "tell application \"Terminal\" to activate"])
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    if cfg!(windows) {
        Command::new("cmd")
            .args(["/C", "start", "", "cmd", "/K", &format!("cd /d {} && {}", shell_quote(&dir_s), command)])
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    let script = format!("cd {} && {}; exec \"${{SHELL:-/bin/sh}}\"", shell_quote(&dir_s), command);
    for (term, flag) in [("x-terminal-emulator", "-e"), ("gnome-terminal", "--"), ("konsole", "-e"), ("xfce4-terminal", "-x"), ("kitty", "--"), ("alacritty", "-e"), ("xterm", "-e")] {
        if Command::new(term).args([flag, "sh", "-c", &script]).env("PATH", child_path()).spawn().is_ok() {
            return Ok(());
        }
    }
    Err(format!("no terminal emulator found; run this yourself in {dir_s}:\n{command}"))
}

/// `login`: sign in to Claude Code. `remote`: continue a session from a phone
/// or another computer through Claude Code's Remote Control.
pub fn open_in_terminal(root: &Path, action: &str, session: Option<&str>, title: &str) -> Result<String, String> {
    let claude = find_claude().ok_or("Claude Code is not installed")?;
    let exe = shell_quote(&claude.display().to_string());
    let command = match action {
        "login" => format!("{exe} auth login"),
        "remote" => {
            let mut c = format!("{exe} --mcp-config {} --strict-mcp-config", shell_quote(&mcp_config()?));
            if let Some(s) = session {
                c.push_str(&format!(" --resume {}", shell_quote(s)));
            }
            c.push_str(&format!(" --remote-control {}", shell_quote(title)));
            c
        }
        "resume" => {
            let mut c = format!("{exe} --mcp-config {} --strict-mcp-config", shell_quote(&mcp_config()?));
            if let Some(s) = session {
                c.push_str(&format!(" --resume {}", shell_quote(s)));
            }
            c
        }
        other => return Err(format!("unknown action {other}")),
    };
    open_terminal(root, &command)?;
    Ok(command)
}
