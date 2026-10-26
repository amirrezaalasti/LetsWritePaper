//! The Claude panel: runs the user's own Claude Code CLI as a streaming
//! subprocess (like the VS Code / Cursor extension), wired to this paper's MCP
//! server so every edit Claude makes is attributed and visible live.
//!
//! Protocol: `claude -p --input-format stream-json --output-format stream-json`
//! — one JSON message per line each way. Sessions are resumable with
//! `--resume <id>`; continuing on another device uses Claude Code's Remote
//! Control (`claude --resume <id> --remote-control`), opened in a terminal.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::{STANDARD as B64, STANDARD_NO_PAD as B64_NOPAD};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

const MAX_ATTACHMENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_ATTACHMENT_TOTAL: usize = 8 * 1024 * 1024;
const MAX_ATTACHMENTS: usize = 8;
const MAX_ATTACHMENT_TEXT: usize = 100_000;

const SYSTEM_PROMPT: &str = "You are working inside LetsWritePaper, a desktop editor for academic papers, as the user's co-author. \
Use the paper's MCP tools (named mcp__paper__<tool>, e.g. mcp__paper__project_overview) for everything about the paper: project_overview to orient yourself; section_context before \
changing a section (follow the skills it returns and consider open review comments); edit_document with exact old_text \
and a clear rationale for focused edits; propose_edit when the user asked for suggestions or the change is substantive; \
compile to check the paper still builds. The user sees your edits appear live in their editor and in the history under \
your name. Never invent citations: cite only keys present in the project's .bib files, otherwise write TODO. \
Keep replies short; the paper itself is where your work goes. \
WebSearch, WebFetch and any other tool that is not pre-approved raise an allow prompt in this chat; wait for the user's choice instead of saying you cannot.";

const GEMINI_PROMPT: &str = "You are working inside LetsWritePaper, a desktop editor for academic papers, as the user's co-author. \
Use the paper MCP server for everything about the paper: project_overview to orient yourself; section_context before \
changing a section (follow the skills it returns and consider open review comments); edit_document with exact old_text \
and a clear rationale for focused edits; propose_edit when the user asked for suggestions or the change is substantive; \
compile to check the paper still builds. Call those tools directly. The user sees your edits appear live in their editor and in the history under \
your name. Never invent citations: cite only keys present in the project's .bib files, otherwise write TODO. \
Keep replies short; the paper itself is where your work goes.";

#[derive(Clone, Debug, Serialize)]
pub struct ClaudeStatus {
    pub installed: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub install_command: String,
}

/// Which account or cloud provider the user signed in with, from `claude auth status`.
#[derive(Clone, Debug, Serialize)]
pub struct ClaudeAuth {
    pub logged_in: bool,
    /// `claudeai`, `console`, `bedrock`, `vertex`, or `foundry`.
    pub provider: String,
    /// Email and organization when Claude Code reports them.
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// `claude`, `chatgpt`, or `gemini`. Older session files are Claude.
    #[serde(default = "default_assistant")]
    pub assistant: String,
}

fn default_assistant() -> String {
    "claude".into()
}

/// What a Gemini chat was started with, so an allow can restart that same chat.
#[derive(Clone)]
struct AgyLaunch {
    root: std::path::PathBuf,
    model: String,
    mode: String,
    use_api_key: bool,
    api_key: String,
}

struct Chat {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    session_id: Arc<Mutex<Option<String>>>,
    assistant: String,
    codex_model: String,
    codex_mode: String,
    /// The `codex exec` process for the turn in flight, if any.
    codex_run: Arc<Mutex<Option<Child>>>,
    /// Bumped when Gemini is restarted so the old reader does not report an exit.
    generation: u64,
    agy: Option<AgyLaunch>,
}

#[derive(Default)]
pub struct ClaudeChats {
    chats: Arc<Mutex<HashMap<String, Chat>>>,
}

/// The user's answer to a `can_use_tool` prompt. Field names match the JSON
/// the panel sends.
#[derive(Debug, Deserialize)]
pub struct PermissionReply {
    pub request_id: String,
    pub behavior: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub updated_input: Option<Value>,
    #[serde(default)]
    pub updated_permissions: Option<Value>,
    #[serde(default)]
    pub decision: Option<String>,
    /// Antigravity allow-rule chosen in the chat, such as `command(echo)`.
    #[serde(default)]
    pub allow_rule: Option<String>,
}

fn write_line(
    chats: &Arc<Mutex<HashMap<String, Chat>>>,
    chat_id: &str,
    line: &str,
) -> Result<(), String> {
    let mut chats = chats.lock().unwrap();
    let chat = chats.get_mut(chat_id).ok_or("this chat is not running")?;
    let stdin = chat
        .stdin
        .as_mut()
        .ok_or("this chat is not accepting input")?;
    writeln!(stdin, "{line}").map_err(|e| format!("Claude Code is not accepting input: {e}"))?;
    stdin.flush().map_err(|e| e.to_string())
}

/// The line the CLI is blocked on. `updatedInput` is always an object: the
/// CLI reads `Object.keys(updatedInput)` and throws if the field is missing.
fn control_response(reply: &PermissionReply) -> Result<Value, String> {
    let decision = match reply.decision.as_deref() {
        Some(d @ ("user_temporary" | "user_permanent" | "user_reject")) => d,
        Some(other) => return Err(format!("unknown permission decision {other}")),
        None => match reply.behavior.as_str() {
            "deny" => "user_reject",
            _ => "user_temporary",
        },
    };
    let mut body = match reply.behavior.as_str() {
        "allow" => {
            let updated_input = match &reply.updated_input {
                Some(Value::Object(_)) => reply.updated_input.clone().unwrap(),
                _ => json!({}),
            };
            json!({
                "behavior": "allow",
                "updatedInput": updated_input,
                "decisionClassification": decision,
            })
        }
        "deny" => {
            let message = if reply.message.trim().is_empty() {
                "The user denied this."
            } else {
                reply.message.as_str()
            };
            json!({
                "behavior": "deny",
                "message": message,
                "decisionClassification": decision,
            })
        }
        other => return Err(format!("unknown permission behavior {other}")),
    };
    if let Some(id) = &reply.tool_use_id {
        body["toolUseID"] = json!(id);
    }
    if reply.behavior == "allow" {
        if let Some(perms) = &reply.updated_permissions {
            if perms.as_array().is_some() {
                body["updatedPermissions"] = perms.clone();
            }
        }
    }
    Ok(json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": reply.request_id,
            "response": body,
        }
    }))
}

/// A control_request this app does not show to the user. The CLI waits forever
/// if nothing answers, so an unknown subtype is refused immediately.
fn refuse_control(request_id: &str, subtype: &str) -> Value {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "error",
            "request_id": request_id,
            "error": format!("LetsWritePaper does not handle {subtype}"),
        }
    })
}

#[derive(Clone, Serialize)]
struct ChatEvent {
    chat_id: String,
    event: Value,
}

#[derive(Deserialize, Default, Clone)]
pub struct ContextMention {
    /// `section` or `file`.
    pub kind: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub section: Option<String>,
    #[serde(default)]
    pub line: Option<usize>,
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct EditorContext {
    pub path: Option<String>,
    pub line: Option<usize>,
    pub section: Option<String>,
    pub selection: Option<String>,
    /// Sections and files the user tagged with ⌘/Ctrl+L or @.
    #[serde(default)]
    pub mentions: Vec<ContextMention>,
}

/// A file pasted or attached in the composer. `data` is standard base64.
#[derive(Debug, Deserialize)]
pub struct ChatAttachment {
    pub name: String,
    pub media_type: String,
    pub data: String,
}

// ---- finding and installing the CLI ------------------------------------------------

/// Directories GUI apps often lack on PATH but installers use.
fn extra_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![];
    if let Some(home) = home_dir() {
        dirs.push(home.join(".local").join("bin"));
        dirs.push(home.join(".claude").join("local"));
        dirs.push(home.join(".npm-global").join("bin"));
        dirs.push(home.join(".bun").join("bin"));
        if cfg!(windows) {
            if let Ok(local) = std::env::var("LOCALAPPDATA") {
                dirs.push(PathBuf::from(local).join("agy").join("bin"));
            }
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

pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// PATH for child processes: the inherited one plus common install locations.
fn child_path() -> std::ffi::OsString {
    let mut paths: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for d in extra_dirs() {
        if !paths.contains(&d) {
            paths.push(d);
        }
    }
    std::env::join_paths(paths).unwrap_or_default()
}

pub fn find_claude() -> Option<PathBuf> {
    find_named(crate::assistants::bin_names("claude"))
}

pub(crate) fn find_named(names: &[&str]) -> Option<PathBuf> {
    let path = child_path();
    for dir in std::env::split_paths(&path) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    // Fall back to the user's login shell, which knows their real PATH.
    if !cfg!(windows) {
        let primary = names
            .iter()
            .copied()
            .find(|name| !name.ends_with(".exe") && !name.ends_with(".cmd"))
            .unwrap_or(names.first().copied().unwrap_or(""));
        if primary.is_empty() {
            return None;
        }
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        if let Ok(out) = Command::new(shell)
            .args(["-lc", &format!("command -v {primary}")])
            .output()
        {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() && Path::new(&p).is_file() {
                return Some(PathBuf::from(p));
            }
        }
    }
    None
}

pub fn status_for(assistant: &str) -> ClaudeStatus {
    let assistant = crate::assistants::normalize(Some(assistant));
    let path = find_named(crate::assistants::bin_names(assistant));
    let version = path.as_ref().and_then(|p| {
        Command::new(p)
            .arg("--version")
            .env("PATH", child_path())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                let text = String::from_utf8_lossy(&o.stdout);
                let text = if text.trim().is_empty() {
                    String::from_utf8_lossy(&o.stderr).into_owned()
                } else {
                    text.into_owned()
                };
                text.lines().next().unwrap_or("").trim().to_string()
            })
            .filter(|s| !s.is_empty())
    });
    ClaudeStatus {
        installed: path.is_some(),
        path: path.map(|p| p.display().to_string()),
        version,
        install_command: crate::assistants::install_command(assistant),
    }
}

/// Map `claude auth status` fields onto the providers the sign-in dialog offers.
pub fn provider_from_status(api_provider: &str, auth_method: &str) -> &'static str {
    let api = api_provider.to_ascii_lowercase();
    let method = auth_method.to_ascii_lowercase();
    if api.contains("bedrock") {
        "bedrock"
    } else if api.contains("vertex") || api.contains("agent platform") {
        "vertex"
    } else if api.contains("foundry") {
        "foundry"
    } else if method.contains("console") || method.contains("api") {
        "console"
    } else {
        "claudeai"
    }
}

pub fn auth_for(assistant: &str) -> ClaudeAuth {
    match crate::assistants::normalize(Some(assistant)) {
        "chatgpt" => codex_auth(),
        "gemini" => gemini_auth(),
        _ => claude_auth(),
    }
}

fn codex_auth() -> ClaudeAuth {
    let unknown = ClaudeAuth {
        logged_in: false,
        provider: "chatgpt".into(),
        detail: String::new(),
    };
    let Some(bin) = find_named(crate::assistants::bin_names("chatgpt")) else {
        return unknown;
    };
    let Ok(out) = Command::new(bin)
        .args(["login", "status"])
        .env("PATH", child_path())
        .output()
    else {
        return unknown;
    };
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    let logged_in = out.status.success()
        && !lower.contains("not logged")
        && !lower.contains("not signed")
        && !text.is_empty();
    let provider = if lower.contains("api key") || lower.contains("api_key") {
        "apikey"
    } else {
        "chatgpt"
    };
    ClaudeAuth {
        logged_in,
        provider: provider.into(),
        detail: text
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(180)
            .collect(),
    }
}

fn gemini_auth() -> ClaudeAuth {
    let installed = find_named(crate::assistants::bin_names("gemini")).is_some();
    if crate::assistants::settings_path()
        .as_deref()
        .is_some_and(crate::assistants::model_provider_is_api)
    {
        return ClaudeAuth {
            logged_in: installed,
            provider: "apikey".into(),
            detail: "Antigravity is set to a Gemini API key".into(),
        };
    }
    ClaudeAuth {
        logged_in: false,
        provider: "google".into(),
        detail: if installed {
            "Sign in with the Google account that has Pro or Ultra".into()
        } else {
            String::new()
        },
    }
}

fn claude_auth() -> ClaudeAuth {
    let unknown = ClaudeAuth {
        logged_in: false,
        provider: "claudeai".into(),
        detail: String::new(),
    };
    let Some(claude) = find_claude() else {
        return unknown;
    };
    let Ok(out) = Command::new(claude)
        .args(["auth", "status"])
        .env("PATH", child_path())
        .output()
    else {
        return unknown;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let Some(value) = parse_json_object(&text) else {
        return unknown;
    };
    let logged_in = value
        .get("loggedIn")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let api = value
        .get("apiProvider")
        .and_then(Value::as_str)
        .unwrap_or("");
    let method = value
        .get("authMethod")
        .and_then(Value::as_str)
        .unwrap_or("");
    let email = value.get("email").and_then(Value::as_str).unwrap_or("");
    let org = value.get("orgName").and_then(Value::as_str).unwrap_or("");
    let detail = match (email.is_empty(), org.is_empty()) {
        (false, false) => format!("{email} · {org}"),
        (false, true) => email.to_string(),
        (true, false) => org.to_string(),
        (true, true) => String::new(),
    };
    ClaudeAuth {
        logged_in,
        provider: provider_from_status(api, method).into(),
        detail,
    }
}

fn parse_json_object(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(&text[start..=end]).ok()
}

/// Run the official installer, streaming its output to the UI.
pub fn install_for(app: &AppHandle, assistant: &str) -> Result<ClaudeStatus, String> {
    let assistant = crate::assistants::normalize(Some(assistant));
    if assistant == "gemini" {
        let app = app.clone();
        crate::assistants::install_antigravity(|line| {
            let _ = app.emit("claude://install-log", line.to_string());
        })?;
        return Ok(status_for(assistant));
    }
    let command = crate::assistants::install_command(assistant);
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("powershell");
        c.args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &command,
        ]);
        c
    } else {
        let mut c = Command::new("/bin/bash");
        c.args(["-c", &command]);
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
    Ok(status_for(assistant))
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

fn touch_session(root: &Path, id: &str, title: Option<&str>, assistant: &str) {
    let mut list = sessions(root);
    let now = paper_core::now_ms();
    match list.iter_mut().find(|s| s.id == id) {
        Some(s) => {
            s.updated_at = now;
            if s.assistant.is_empty() {
                s.assistant = assistant.to_string();
            }
            if s.title.is_empty() {
                if let Some(t) = title {
                    s.title = t.to_string();
                }
            }
        }
        None => list.push(SessionInfo {
            id: id.into(),
            title: title.unwrap_or("").into(),
            created_at: now,
            updated_at: now,
            assistant: assistant.to_string(),
        }),
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
    cmd.current_dir(root)
        .env("PATH", child_path())
        .env("PAPER_AGENT", "claude-code");
    // Only the paper's server: faster start-up and no unrelated tools.
    cmd.args(["--mcp-config", &mcp_config()?, "--strict-mcp-config"]);
    Ok(cmd)
}

impl ClaudeChats {
    /// Start (or resume) a chat. `mode`: `paper` (paper tools and reading are
    /// pre-approved; anything else asks in the chat) or `edits` (also
    /// auto-accepts file edits).
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &self,
        app: &AppHandle,
        root: &Path,
        chat_id: &str,
        resume: Option<&str>,
        mode: &str,
        model: Option<&str>,
        provider: Option<&str>,
        foundry_resource: Option<&str>,
        foundry_api_key: Option<&str>,
        assistant: Option<&str>,
        api_key: Option<&str>,
    ) -> Result<(), String> {
        self.stop(chat_id);
        match crate::assistants::normalize(assistant) {
            "chatgpt" => return self.start_codex(chat_id, resume, mode, model),
            "gemini" => {
                return self.start_agy(app, root, chat_id, resume, mode, model, provider, api_key)
            }
            _ => {}
        }
        let claude = find_claude().ok_or("Claude Code is not installed")?;
        let mut cmd = base_args(&claude, root)?;
        if let Some(provider) = provider {
            apply_provider(&mut cmd, provider, foundry_resource, foundry_api_key);
        }
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
            // Headless mode has no terminal, so it would deny every other tool.
            // `stdio` is the Claude Agent SDK's name for "ask the parent": each
            // approval arrives as a control_request and waits for a control_response.
            "--permission-prompt-tool",
            "stdio",
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
        // Registered before the reader starts: a permission request can arrive
        // before the first user message, and answering it needs this stdin.
        self.chats.lock().unwrap().insert(
            chat_id.to_string(),
            Chat {
                child: Some(child),
                stdin: Some(stdin),
                session_id: session_id.clone(),
                assistant: "claude".into(),
                codex_model: String::new(),
                codex_mode: String::new(),
                codex_run: Arc::new(Mutex::new(None)),
                generation: 0,
                agy: None,
            },
        );

        let (app2, id2, root2, sid2, chats2) = (
            app.clone(),
            chat_id.to_string(),
            root.to_path_buf(),
            session_id,
            Arc::clone(&self.chats),
        );
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Ok(event) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if event.get("type").and_then(Value::as_str) == Some("system")
                    && event.get("subtype").and_then(Value::as_str) == Some("init")
                {
                    if let Some(sid) = event.get("session_id").and_then(Value::as_str) {
                        *sid2.lock().unwrap() = Some(sid.to_string());
                        touch_session(&root2, sid, None, "claude");
                    }
                }
                // The CLI blocks on every control_request until some response
                // with that id comes back. Permission prompts are answered by
                // the panel. Anything else is refused here, because silence
                // stalls the turn until the chat is killed.
                if event.get("type").and_then(Value::as_str) == Some("control_request") {
                    let subtype = event
                        .pointer("/request/subtype")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    if subtype != "can_use_tool" {
                        if let Some(rid) = event.get("request_id").and_then(Value::as_str) {
                            if let Ok(body) = serde_json::to_string(&refuse_control(rid, subtype)) {
                                let _ = write_line(&chats2, &id2, &body);
                            }
                        }
                    }
                }
                let _ = app2.emit(
                    "claude://event",
                    ChatEvent {
                        chat_id: id2.clone(),
                        event,
                    },
                );
            }
            let _ = app2.emit(
                "claude://event",
                ChatEvent {
                    chat_id: id2.clone(),
                    event: json!({ "type": "exit" }),
                },
            );
        });
        if let Some(stderr) = stderr {
            let (app3, id3) = (app.clone(), chat_id.to_string());
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let _ = app3.emit(
                        "claude://event",
                        ChatEvent {
                            chat_id: id3.clone(),
                            event: json!({ "type": "stderr", "text": line }),
                        },
                    );
                }
            });
        }
        Ok(())
    }

    /// Write the user's allow or deny back to the CLI, which is blocked on it.
    /// Gemini has no blocking prompt: an allow is saved and the same chat is
    /// restarted so the new process actually grants it.
    pub fn permission(
        &self,
        app: &AppHandle,
        chat_id: &str,
        reply: &PermissionReply,
    ) -> Result<(), String> {
        let assistant = {
            let chats = self.chats.lock().unwrap();
            chats
                .get(chat_id)
                .map(|chat| chat.assistant.clone())
                .ok_or("this chat is not running")?
        };
        if assistant == "gemini" {
            return self.gemini_permission(app, chat_id, reply);
        }
        let line = serde_json::to_string(&control_response(reply)?).map_err(|e| e.to_string())?;
        write_line(&self.chats, chat_id, &line)
    }

    fn gemini_permission(
        &self,
        app: &AppHandle,
        chat_id: &str,
        reply: &PermissionReply,
    ) -> Result<(), String> {
        if reply.behavior != "allow" {
            return Ok(());
        }
        let rule = reply
            .allow_rule
            .as_deref()
            .map(str::trim)
            .filter(|rule| !rule.is_empty())
            .ok_or("This permission has no allow rule.")?;
        let path =
            crate::assistants::settings_path().ok_or("Could not find Antigravity settings.")?;
        crate::assistants::grant_allow_rule(&path, rule)?;
        let retry = format!(
            "The user allowed `{rule}`. Retry the action that was just denied. Do not ask again."
        );
        self.restart_agy(app, chat_id, &retry)
    }

    fn start_codex(
        &self,
        chat_id: &str,
        resume: Option<&str>,
        mode: &str,
        model: Option<&str>,
    ) -> Result<(), String> {
        if find_named(crate::assistants::bin_names("chatgpt")).is_none() {
            return Err("Codex is not installed".into());
        }
        self.chats.lock().unwrap().insert(
            chat_id.to_string(),
            Chat {
                child: None,
                stdin: None,
                session_id: Arc::new(Mutex::new(resume.map(str::to_string))),
                assistant: "chatgpt".into(),
                codex_model: model.unwrap_or("").to_string(),
                codex_mode: mode.to_string(),
                codex_run: Arc::new(Mutex::new(None)),
                generation: 0,
                agy: None,
            },
        );
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn start_agy(
        &self,
        app: &AppHandle,
        root: &Path,
        chat_id: &str,
        resume: Option<&str>,
        mode: &str,
        model: Option<&str>,
        provider: Option<&str>,
        api_key: Option<&str>,
    ) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        crate::assistants::ensure_workspace_mcp(root, &exe.display().to_string(), "gemini")?;
        let use_api_key = provider == Some("apikey");
        if let Some(path) = crate::assistants::settings_path() {
            crate::assistants::set_model_provider(&path, use_api_key)?;
            crate::assistants::grant_allow_rule(&path, crate::assistants::PAPER_ALLOW_RULE)?;
        }
        let launch = AgyLaunch {
            root: root.to_path_buf(),
            model: model.unwrap_or("").to_string(),
            mode: mode.to_string(),
            use_api_key,
            api_key: nonempty(api_key).unwrap_or("").to_string(),
        };
        let session_id = Arc::new(Mutex::new(resume.map(str::to_string)));
        let generation = 1;
        let (stdin, stdout, stderr, child) = spawn_agy_child(&launch, resume)?;
        self.chats.lock().unwrap().insert(
            chat_id.to_string(),
            Chat {
                child: Some(child),
                stdin: Some(stdin),
                session_id: session_id.clone(),
                assistant: "gemini".into(),
                codex_model: String::new(),
                codex_mode: mode.to_string(),
                codex_run: Arc::new(Mutex::new(None)),
                generation,
                agy: Some(launch),
            },
        );
        watch_output(
            app,
            chat_id,
            root,
            stdout,
            stderr,
            session_id,
            Arc::clone(&self.chats),
            generation,
        );
        Ok(())
    }

    /// Stop the current Gemini process and start it again on the same
    /// conversation, then send `retry`. A saved allow-rule is only read at
    /// startup, so the retry has to be a new process.
    fn restart_agy(&self, app: &AppHandle, chat_id: &str, retry: &str) -> Result<(), String> {
        let (launch, session_id, generation, child) = {
            let mut chats = self.chats.lock().unwrap();
            let chat = chats.get_mut(chat_id).ok_or("this chat is not running")?;
            let launch = chat.agy.clone().ok_or("this chat is not Gemini")?;
            chat.generation = chat.generation.wrapping_add(1);
            let generation = chat.generation;
            let session_id = Arc::clone(&chat.session_id);
            let child = chat.child.take();
            chat.stdin.take();
            (launch, session_id, generation, child)
        };
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(path) = crate::assistants::settings_path() {
            crate::assistants::grant_allow_rule(&path, crate::assistants::PAPER_ALLOW_RULE)?;
        }
        let resume = session_id.lock().unwrap().clone();
        let (stdin, stdout, stderr, child) = spawn_agy_child(&launch, resume.as_deref())?;
        {
            let mut chats = self.chats.lock().unwrap();
            let chat = chats.get_mut(chat_id).ok_or("this chat is not running")?;
            if chat.generation != generation {
                drop(stdin);
                drop(stdout);
                drop(stderr);
                let mut child = child;
                let _ = child.kill();
                let _ = child.wait();
                return Err("this chat restarted".into());
            }
            chat.child = Some(child);
            chat.stdin = Some(stdin);
        }
        watch_output(
            app,
            chat_id,
            &launch.root,
            stdout,
            stderr,
            session_id,
            Arc::clone(&self.chats),
            generation,
        );
        let line = serde_json::to_string(&json!({
            "event": "user",
            "message": { "content": retry }
        }))
        .map_err(|e| e.to_string())?;
        write_line(&self.chats, chat_id, &line)
    }

    pub fn send(
        &self,
        app: &AppHandle,
        root: &Path,
        chat_id: &str,
        text: &str,
        ctx: &EditorContext,
        attachments: &[ChatAttachment],
    ) -> Result<(), String> {
        let assistant = {
            let chats = self.chats.lock().unwrap();
            chats
                .get(chat_id)
                .map(|chat| chat.assistant.clone())
                .ok_or("this chat is not running")?
        };
        if text.trim().is_empty() && attachments.is_empty() {
            return Err("Write a message or attach a file.".into());
        }
        slash_for_assistant(&assistant, text)?;
        if assistant == "claude" && attachments.is_empty() {
            if let Some(answer) = self.local_slash_answer(root, chat_id, text) {
                emit_local_answer(app, chat_id, &answer);
                return Ok(());
            }
        }
        let files = prepare_attachments(root, attachments)?;
        if assistant == "chatgpt" {
            return self.send_codex(app, root, chat_id, text, ctx, &files);
        }
        let line = outbound_line(&assistant, text, ctx, &files);
        let mut chats = self.chats.lock().unwrap();
        let chat = chats.get_mut(chat_id).ok_or("this chat is not running")?;
        let stdin = chat
            .stdin
            .as_mut()
            .ok_or("this chat is not accepting input")?;
        writeln!(stdin, "{line}")
            .map_err(|e| format!("the assistant is not accepting input: {e}"))?;
        stdin.flush().map_err(|e| e.to_string())?;
        if let Some(sid) = chat.session_id.lock().unwrap().clone() {
            touch_session(root, &sid, Some(&message_title(text, &files)), &assistant);
        }
        Ok(())
    }

    fn send_codex(
        &self,
        app: &AppHandle,
        root: &Path,
        chat_id: &str,
        text: &str,
        ctx: &EditorContext,
        files: &[PreparedFile],
    ) -> Result<(), String> {
        let (model, mode, resume, run, session_id) = {
            let chats = self.chats.lock().unwrap();
            let chat = chats.get(chat_id).ok_or("this chat is not running")?;
            if chat.codex_run.lock().unwrap().is_some() {
                return Err("still working on the previous message".into());
            }
            let model = chat.codex_model.clone();
            let mode = chat.codex_mode.clone();
            let resume = chat.session_id.lock().unwrap().clone();
            let run = Arc::clone(&chat.codex_run);
            let session_id = Arc::clone(&chat.session_id);
            (model, mode, resume, run, session_id)
        };
        let bin =
            find_named(crate::assistants::bin_names("chatgpt")).ok_or("Codex is not installed")?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let images: Vec<String> = files
            .iter()
            .filter(|file| file.kind == AttachKind::Image)
            .map(|file| file.abs.display().to_string())
            .collect();
        let mut cmd = Command::new(bin);
        cmd.current_dir(root)
            .env("PATH", child_path())
            .env("PAPER_AGENT", "codex")
            .args(crate::assistants::codex_args(
                &exe.display().to_string(),
                resume.as_deref(),
                &mode,
                Some(&model),
                &images,
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("could not start Codex: {e}"))?;
        let prompt = prompt_for("chatgpt", text, ctx, files);
        if let Some(mut stdin) = child.stdin.take() {
            writeln!(stdin, "{prompt}").map_err(|e| e.to_string())?;
        }
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take();
        *run.lock().unwrap() = Some(child);
        let app = app.clone();
        let root = root.to_path_buf();
        let chat_id = chat_id.to_string();
        let title = message_title(text, files);
        let model = model.clone();
        let stderr_buf = Arc::new(Mutex::new(Vec::<String>::new()));
        let stderr_handle = stderr.map(|stderr| {
            let buf = Arc::clone(&stderr_buf);
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    buf.lock().unwrap().push(line);
                }
            })
        });
        std::thread::spawn(move || {
            let mut saw_result = false;
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Ok(event) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                for translated in crate::assistants::translate_codex(&event, &model) {
                    if translated.get("type").and_then(Value::as_str) == Some("result") {
                        saw_result = true;
                    }
                    if let Some(sid) = translated.get("session_id").and_then(Value::as_str) {
                        *session_id.lock().unwrap() = Some(sid.to_string());
                        touch_session(&root, sid, Some(&title), "chatgpt");
                    }
                    let _ = app.emit(
                        "claude://event",
                        ChatEvent {
                            chat_id: chat_id.clone(),
                            event: translated,
                        },
                    );
                }
            }
            if let Some(mut child) = run.lock().unwrap().take() {
                let _ = child.wait();
            }
            if let Some(handle) = stderr_handle {
                let _ = handle.join();
            }
            if !saw_result {
                let stderr_lines = stderr_buf.lock().unwrap().clone();
                let detail = if stderr_lines.is_empty() {
                    "ChatGPT ended without a reply".to_string()
                } else {
                    stderr_lines.join("\n")
                };
                let _ = app.emit(
                    "claude://event",
                    ChatEvent {
                        chat_id,
                        event: json!({ "type": "result", "is_error": true, "result": detail }),
                    },
                );
            }
        });
        Ok(())
    }

    pub fn stop(&self, chat_id: &str) -> Option<String> {
        let mut chats = self.chats.lock().unwrap();
        let mut chat = chats.remove(chat_id)?;
        if let Some(mut child) = chat.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(mut child) = chat.codex_run.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let sid = chat.session_id.lock().unwrap().clone();
        sid
    }

    /// `/usage` and `/status` are screens in Claude Code's terminal. This chat
    /// has no terminal, so the panel answers them from the signed-in account.
    /// Plugin, skill, and settings screens are opened in a terminal instead of
    /// being sent, where this Claude build would reply "Unknown command".
    fn local_slash_answer(&self, root: &Path, chat_id: &str, text: &str) -> Option<String> {
        let name = slash_name(text)?;
        match name {
            "usage" | "cost" | "stats" | "usage-credits" | "extra-usage" => {
                Some(plan_usage_report())
            }
            "status" => Some(status_report()),
            "plugin" | "skills" | "doctor" | "checkup" | "help" | "mcp" | "permissions"
            | "allowed-tools" | "memory" | "diff" | "reload-plugins" | "reload-skills"
            | "statusline" => {
                let session = {
                    let chats = self.chats.lock().unwrap();
                    chats
                        .get(chat_id)
                        .and_then(|chat| chat.session_id.lock().unwrap().clone())
                };
                let opened = open_in_terminal(
                    root,
                    "resume",
                    session.as_deref(),
                    "LetsWritePaper",
                    None,
                    None,
                    Some("claude"),
                );
                Some(terminal_slash_message(name, opened.err().as_deref()))
            }
            _ => None,
        }
    }

    pub fn stop_all(&self) {
        let ids: Vec<String> = self.chats.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.stop(&id);
        }
    }
}

fn message_text(text: &str, ctx: &EditorContext, instructions: Option<&str>) -> String {
    let mut content = String::new();
    if let Some(prompt) = instructions {
        content.push_str(prompt);
        content.push_str("\n\n");
    }
    let mentions = mention_block(ctx);
    if ctx.path.is_some() || ctx.selection.is_some() || !mentions.is_empty() {
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
        content.push_str(&mentions);
        content.push_str("</editor_context>\n\n");
    }
    content.push_str(text);
    content
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AttachKind {
    Image,
    Pdf,
    Text,
    Other,
}

#[derive(Debug)]
struct PreparedFile {
    name: String,
    rel: String,
    abs: PathBuf,
    kind: AttachKind,
    /// Normalized MIME for an image or PDF block. Empty otherwise.
    media: String,
    /// Base64 payload for an image or PDF block.
    data: String,
    text: Option<String>,
}

fn slash_for_assistant(assistant: &str, text: &str) -> Result<(), String> {
    if is_slash_command(text) && assistant != "claude" {
        Err("Commands such as /usage, /status, and /plugin are part of Claude. Switch to Claude with the key button.".into())
    } else {
        Ok(())
    }
}

/// A Claude Code command is recognized only when it is the start of the message.
fn is_slash_command(text: &str) -> bool {
    let first = text.trim().lines().next().unwrap_or("").trim();
    let token = first.split_whitespace().next().unwrap_or("");
    let mut chars = token.chars();
    chars.next() == Some('/') && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
}

/// The command name without the leading slash, when the message is a command.
fn slash_name(text: &str) -> Option<&str> {
    if !is_slash_command(text) {
        return None;
    }
    let token = text.trim().lines().next()?.split_whitespace().next()?;
    Some(token.trim_start_matches('/'))
}

fn emit_local_answer(app: &AppHandle, chat_id: &str, text: &str) {
    let id = format!(
        "local-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    let _ = app.emit(
        "claude://event",
        ChatEvent {
            chat_id: chat_id.to_string(),
            event: json!({
                "type": "assistant",
                "message": { "id": id, "content": [{ "type": "text", "text": text }] },
            }),
        },
    );
    let _ = app.emit(
        "claude://event",
        ChatEvent {
            chat_id: chat_id.to_string(),
            event: json!({ "type": "result", "is_error": false }),
        },
    );
}

fn terminal_slash_message(name: &str, open_error: Option<&str>) -> String {
    let command = format!("/{name}");
    match open_error {
        None => format!(
            "{command} runs in Claude Code's own terminal, which is now open on this paper. Type {command} there."
        ),
        Some(error) => format!(
            "{command} runs in Claude Code's own terminal. The keyboard button in this panel opens it. {error}"
        ),
    }
}

fn status_report() -> String {
    let status = status_for("claude");
    let auth = auth_for("claude");
    let mut lines = Vec::new();
    if let Some(version) = status.version.filter(|v| !v.is_empty()) {
        lines.push(format!("Claude Code {version}"));
    } else {
        lines.push("Claude Code".into());
    }
    if auth.logged_in {
        let who = if auth.detail.is_empty() {
            auth.provider
        } else {
            auth.detail
        };
        lines.push(format!("Signed in: {who}"));
    }
    lines.push(String::new());
    lines.push(plan_usage_report());
    lines.join("\n")
}

fn plan_usage_report() -> String {
    let creds = match read_claude_account() {
        Ok(creds) => creds,
        Err(error) => return error,
    };
    let Some(token) = creds.token.filter(|token| !token.is_empty()) else {
        return "This Claude login has no subscription usage to show. An API-key login is billed per request.".into();
    };
    match fetch_plan_usage(&token) {
        Ok(body) => format_plan_usage(&body, &creds.plan),
        Err(error) => format!("Plan usage could not be loaded. {error}"),
    }
}

struct ClaudeAccount {
    token: Option<String>,
    plan: String,
}

fn read_claude_account() -> Result<ClaudeAccount, String> {
    let value = read_claude_credentials()?;
    let oauth = value.get("claudeAiOauth").cloned().unwrap_or(Value::Null);
    let plan = oauth
        .get("subscriptionType")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let token = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(ClaudeAccount { token, plan })
}

fn read_claude_credentials() -> Result<Value, String> {
    if let Some(home) = home_dir() {
        let path = home.join(".claude").join(".credentials.json");
        if let Ok(text) = fs::read_to_string(path) {
            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                return Ok(value);
            }
        }
    }
    if cfg!(target_os = "macos") {
        let output = Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                "Claude Code-credentials",
                "-w",
            ])
            .output()
            .map_err(|_| "Plan usage could not be read from this Mac.".to_string())?;
        if !output.status.success() {
            return Err("Claude Code has no saved account on this Mac.".into());
        }
        let text = String::from_utf8_lossy(&output.stdout);
        return serde_json::from_str(text.trim())
            .map_err(|_| "Claude Code's saved account could not be read.".to_string());
    }
    Err("Claude Code has no saved account to read plan usage from.".into())
}

/// The token stays in a curl config file, not in the process arguments.
fn fetch_plan_usage(token: &str) -> Result<Value, String> {
    let path = std::env::temp_dir().join(format!(
        "lwp-usage-{}-{}.curl",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let config = format!(
        "silent\nshow-error\nmax-time = 15\nheader = \"Authorization: Bearer {token}\"\nheader = \"anthropic-beta: oauth-2025-04-20\"\nheader = \"Accept: application/json\"\nheader = \"User-Agent: claude-code/letswritepaper\"\nurl = \"https://api.anthropic.com/api/oauth/usage\"\n"
    );
    fs::write(&path, &config).map_err(|_| "Plan usage could not be requested.".to_string())?;
    let _secret = SecretFile(path.clone());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    let output = Command::new("curl")
        .arg("--config")
        .arg(&path)
        .output()
        .map_err(|_| "Plan usage could not be requested.".to_string())?;
    if !output.status.success() {
        return Err("Claude's usage service did not respond.".into());
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|_| "Claude's usage service sent an unexpected reply.".into())
}

fn format_plan_usage(body: &Value, plan: &str) -> String {
    let mut lines = vec!["**Plan usage**".to_string()];
    if let Some(label) = plan_label(plan) {
        lines[0] = format!("**Plan usage** · {label}");
    }
    if let Some(limits) = body.get("limits").and_then(Value::as_array) {
        for limit in limits {
            let Some(kind) = limit.get("kind").and_then(Value::as_str) else {
                continue;
            };
            let Some(label) = limit_label(kind) else {
                continue;
            };
            let percent = limit.get("percent").and_then(Value::as_f64).unwrap_or(0.0);
            let reset = limit
                .get("resets_at")
                .and_then(Value::as_str)
                .map(format_reset)
                .unwrap_or_default();
            if reset.is_empty() {
                lines.push(format!("- {label}: {} used", percent_text(percent)));
            } else {
                lines.push(format!(
                    "- {label}: {} used, resets {reset}",
                    percent_text(percent)
                ));
            }
        }
    }
    if lines.len() == 1 {
        for (key, label) in [
            ("five_hour", "Session limit"),
            ("seven_day", "Weekly limit"),
        ] {
            if let Some(window) = body.get(key) {
                if let Some(line) = window_line(label, window) {
                    lines.push(line);
                }
            }
        }
    }
    if let Some(extra) = extra_usage_line(body.get("extra_usage")) {
        lines.push(extra);
    }
    if lines.len() == 1 {
        lines.push("Claude did not report session or weekly limits for this account.".into());
    }
    lines.join("\n")
}

fn window_line(label: &str, window: &Value) -> Option<String> {
    let percent = window.get("utilization").and_then(Value::as_f64)?;
    let reset = window
        .get("resets_at")
        .and_then(Value::as_str)
        .map(format_reset)
        .unwrap_or_default();
    if reset.is_empty() {
        Some(format!("- {label}: {} used", percent_text(percent)))
    } else {
        Some(format!(
            "- {label}: {} used, resets {reset}",
            percent_text(percent)
        ))
    }
}

fn extra_usage_line(extra: Option<&Value>) -> Option<String> {
    let extra = extra?;
    if extra.get("is_enabled").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    let places = extra
        .get("decimal_places")
        .and_then(Value::as_u64)
        .unwrap_or(2) as u32;
    let currency = extra
        .get("currency")
        .and_then(Value::as_str)
        .unwrap_or("USD");
    let used = extra.get("used_credits").and_then(Value::as_f64)?;
    let limit = extra.get("monthly_limit").and_then(Value::as_f64);
    let used_text = money(used, places, currency);
    let percent = extra
        .get("utilization")
        .and_then(Value::as_f64)
        .map(|value| format!(", {}", percent_text(value)))
        .unwrap_or_default();
    match limit {
        Some(limit) => Some(format!(
            "- Extra usage: {used_text} of {}{percent}",
            money(limit, places, currency)
        )),
        None => Some(format!("- Extra usage: {used_text} used{percent}")),
    }
}

fn limit_label(kind: &str) -> Option<&'static str> {
    match kind {
        "session" => Some("Session limit"),
        "weekly_all" | "weekly" => Some("Weekly limit"),
        "seven_day_opus" | "opus" => Some("Opus weekly limit"),
        "seven_day_sonnet" | "sonnet" => Some("Sonnet weekly limit"),
        _ => None,
    }
}

fn plan_label(plan: &str) -> Option<&'static str> {
    match plan {
        "pro" => Some("Pro"),
        "max" => Some("Max"),
        "team" => Some("Team"),
        "enterprise" => Some("Enterprise"),
        "ultra" => Some("Ultra"),
        _ => None,
    }
}

fn percent_text(value: f64) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{:.0}%", value.round())
    } else {
        format!("{value:.1}%")
    }
}

fn money(amount: f64, places: u32, currency: &str) -> String {
    let scale = 10f64.powi(places as i32);
    let value = if places == 0 { amount } else { amount / scale };
    if currency.eq_ignore_ascii_case("USD") {
        format!("${value:.2}")
    } else {
        format!("{value:.2} {currency}")
    }
}

struct SecretFile(PathBuf);

impl Drop for SecretFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn format_reset(raw: &str) -> String {
    let date = raw.get(..10).unwrap_or(raw);
    let time = raw.get(11..16).unwrap_or("");
    let zone = if raw.ends_with('Z') || raw.contains("+00:00") {
        " UTC"
    } else {
        ""
    };
    if time.is_empty() {
        date.to_string()
    } else {
        format!("{date} {time}{zone}")
    }
}

fn mention_block(ctx: &EditorContext) -> String {
    let known: Vec<&ContextMention> = ctx
        .mentions
        .iter()
        .filter(|mention| mention.kind == "section" || mention.kind == "file")
        .collect();
    if known.is_empty() {
        return String::new();
    }
    let mut out = String::from("The user tagged:\n");
    for mention in known {
        let path = mention.path.as_deref().unwrap_or("(unknown file)");
        if mention.kind == "section" {
            let section = mention.section.as_deref().unwrap_or("(untitled)");
            out.push_str(&format!("- section \"{section}\" in {path}"));
        } else {
            out.push_str(&format!("- file {path}"));
        }
        if let Some(line) = mention.line {
            out.push_str(&format!(" near line {line}"));
        }
        out.push('\n');
        if let Some(text) = mention.text.as_ref().filter(|text| !text.trim().is_empty()) {
            out.push_str(if mention.kind == "section" {
                "  Selected text in that section:\n```\n"
            } else {
                "  Selected text:\n```\n"
            });
            out.push_str(&clip_mention(text));
            out.push_str("\n```\n");
        }
    }
    out
}

fn clip_mention(text: &str) -> String {
    let mut chars = text.chars();
    let clipped: String = chars.by_ref().take(8000).collect();
    if chars.next().is_some() {
        format!("{clipped}\n…(truncated)")
    } else {
        clipped
    }
}

fn message_title(text: &str, files: &[PreparedFile]) -> String {
    let spoken = text.trim();
    if !spoken.is_empty() {
        return spoken.chars().take(60).collect();
    }
    files
        .first()
        .map(|file| file.name.chars().take(60).collect())
        .unwrap_or_default()
}

fn prompt_for(assistant: &str, text: &str, ctx: &EditorContext, files: &[PreparedFile]) -> String {
    let slash = assistant == "claude" && is_slash_command(text);
    let spoken = if text.trim().is_empty() {
        "Please look at the attached files."
    } else {
        text
    };
    let instructions = match assistant {
        "claude" => None,
        "gemini" => Some(GEMINI_PROMPT),
        _ => Some(SYSTEM_PROMPT),
    };
    let mut body = if slash {
        text.trim().to_string()
    } else {
        message_text(spoken, ctx, instructions)
    };
    if !slash {
        body.push_str(&attachment_note(files));
    }
    body
}

fn outbound_line(
    assistant: &str,
    text: &str,
    ctx: &EditorContext,
    files: &[PreparedFile],
) -> Value {
    let body = prompt_for(assistant, text, ctx, files);
    if assistant == "gemini" {
        return json!({ "event": "user", "message": { "content": body } });
    }
    let slash = is_slash_command(text);
    let mut blocks = vec![json!({ "type": "text", "text": body })];
    if !slash {
        for file in files {
            match file.kind {
                AttachKind::Image => blocks.push(json!({
                    "type": "image",
                    "source": { "type": "base64", "media_type": file.media, "data": file.data },
                })),
                AttachKind::Pdf => blocks.push(json!({
                    "type": "document",
                    "source": { "type": "base64", "media_type": "application/pdf", "data": file.data },
                })),
                AttachKind::Text | AttachKind::Other => {}
            }
        }
    }
    json!({
        "type": "user",
        "message": { "role": "user", "content": blocks },
        "parent_tool_use_id": null,
    })
}

fn attachment_note(files: &[PreparedFile]) -> String {
    if files.is_empty() {
        return String::new();
    }
    let mut note = String::from("\n\nAttached files:\n");
    for file in files {
        note.push_str(&format!(
            "- {} ({}, {})\n",
            file.name,
            kind_label(file.kind),
            file.rel
        ));
        if let Some(text) = &file.text {
            note.push_str(&format!("Contents of {}:\n```\n{text}\n```\n", file.name));
        }
    }
    note
}

fn kind_label(kind: AttachKind) -> &'static str {
    match kind {
        AttachKind::Image => "image",
        AttachKind::Pdf => "pdf",
        AttachKind::Text => "text",
        AttachKind::Other => "file",
    }
}

fn prepare_attachments(
    root: &Path,
    attachments: &[ChatAttachment],
) -> Result<Vec<PreparedFile>, String> {
    if attachments.is_empty() {
        return Ok(Vec::new());
    }
    if attachments.len() > MAX_ATTACHMENTS {
        return Err("You can attach up to 8 files.".into());
    }
    let dir = root.join(".paper").join("attachments");
    fs::create_dir_all(&dir).map_err(|e| format!("could not save the attachment: {e}"))?;
    ignore_attachments(root);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut total = 0usize;
    let mut prepared = Vec::with_capacity(attachments.len());
    for (index, attachment) in attachments.iter().enumerate() {
        let name = display_name(&attachment.name);
        let bytes = decode_attachment(&name, &attachment.data)?;
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(format!("{name} is larger than 4 MB"));
        }
        if total + bytes.len() > MAX_ATTACHMENT_TOTAL {
            return Err("Attachments are limited to 8 MB altogether.".into());
        }
        total += bytes.len();
        let filename = format!("{stamp}-{index}-{}", safe_file_name(&name));
        let abs = dir.join(&filename);
        fs::write(&abs, &bytes).map_err(|e| format!("could not save {name}: {e}"))?;
        let rel = format!(".paper/attachments/{filename}");
        let (kind, media, data, text) = classify(&name, &attachment.media_type, &bytes);
        prepared.push(PreparedFile {
            name,
            rel,
            abs,
            kind,
            media,
            data,
            text,
        });
    }
    Ok(prepared)
}

fn decode_attachment(name: &str, data: &str) -> Result<Vec<u8>, String> {
    let data: String = data.chars().filter(|c| !c.is_whitespace()).collect();
    if data.is_empty() {
        return Err(format!("{name} is empty"));
    }
    B64.decode(&data)
        .or_else(|_| B64_NOPAD.decode(&data))
        .map_err(|_| format!("{name} could not be read"))
}

fn classify(
    name: &str,
    media_type: &str,
    bytes: &[u8],
) -> (AttachKind, String, String, Option<String>) {
    if let Some(media) = image_type(media_type, name) {
        return (
            AttachKind::Image,
            media.to_string(),
            B64.encode(bytes),
            None,
        );
    }
    if is_pdf(media_type, name) {
        return (
            AttachKind::Pdf,
            "application/pdf".into(),
            B64.encode(bytes),
            None,
        );
    }
    if is_text_file(media_type, name) {
        if let Ok(text) = std::str::from_utf8(bytes) {
            return (
                AttachKind::Text,
                String::new(),
                String::new(),
                Some(clip_text(text)),
            );
        }
    }
    (AttachKind::Other, String::new(), String::new(), None)
}

fn image_type(media_type: &str, name: &str) -> Option<&'static str> {
    match media_type.trim().to_ascii_lowercase().as_str() {
        "image/jpeg" | "image/jpg" => return Some("image/jpeg"),
        "image/png" => return Some("image/png"),
        "image/gif" => return Some("image/gif"),
        "image/webp" => return Some("image/webp"),
        _ => {}
    }
    match extension(name).as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn is_pdf(media_type: &str, name: &str) -> bool {
    media_type.trim().eq_ignore_ascii_case("application/pdf") || extension(name) == "pdf"
}

fn is_text_file(media_type: &str, name: &str) -> bool {
    let media = media_type.trim().to_ascii_lowercase();
    if media.starts_with("text/")
        || media == "application/json"
        || media == "application/xml"
        || media == "application/javascript"
        || media == "application/x-tex"
        || media == "image/svg+xml"
    {
        return true;
    }
    matches!(
        extension(name).as_str(),
        "txt"
            | "md"
            | "markdown"
            | "tex"
            | "ltx"
            | "typ"
            | "bib"
            | "csv"
            | "tsv"
            | "json"
            | "jsonl"
            | "toml"
            | "yml"
            | "yaml"
            | "xml"
            | "html"
            | "css"
            | "js"
            | "ts"
            | "tsx"
            | "jsx"
            | "py"
            | "rs"
            | "sh"
            | "sty"
            | "cls"
            | "bst"
            | "svg"
            | "log"
    )
}

fn clip_text(text: &str) -> String {
    match text.char_indices().nth(MAX_ATTACHMENT_TEXT) {
        Some((index, _)) => format!("{}\n… [truncated]", &text[..index]),
        None => text.to_string(),
    }
}

fn extension(name: &str) -> String {
    name.rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

fn ignore_attachments(root: &Path) {
    let path = root.join(".gitignore");
    let Ok(existing) = fs::read_to_string(&path) else {
        return;
    };
    if existing.lines().any(|line| {
        let line = line.trim();
        line == ".paper/attachments" || line == ".paper/attachments/"
    }) {
        return;
    }
    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(".paper/attachments/\n");
    let _ = fs::write(path, next);
}

fn display_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    if base.is_empty() {
        "file".into()
    } else {
        base.to_string()
    }
}

fn safe_file_name(name: &str) -> String {
    let mut clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    while clean.starts_with('.') {
        clean.remove(0);
    }
    if clean.is_empty() {
        clean = "file".into();
    }
    if clean.len() > 80 {
        clean.truncate(80);
    }
    clean
}

fn spawn_agy_child(
    launch: &AgyLaunch,
    resume: Option<&str>,
) -> Result<(ChildStdin, ChildStdout, Option<ChildStderr>, Child), String> {
    let agy =
        find_named(crate::assistants::bin_names("gemini")).ok_or("Antigravity is not installed")?;
    let mut cmd = Command::new(agy);
    cmd.current_dir(&launch.root)
        .env("PATH", child_path())
        .env("PAPER_AGENT", "gemini")
        .args(crate::assistants::agy_args(
            resume,
            nonempty(Some(&launch.model)),
            &launch.mode,
        ));
    if launch.use_api_key && !launch.api_key.is_empty() {
        cmd.env("GEMINI_API_KEY", &launch.api_key);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start Antigravity: {e}"))?;
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take();
    Ok((stdin, stdout, stderr, child))
}

/// Read an Antigravity stream and emit it as panel events.
/// `generation` belongs to this process. A newer one means the chat restarted
/// on purpose, so this reader must not tell the panel the chat exited.
fn watch_output(
    app: &AppHandle,
    chat_id: &str,
    root: &Path,
    stdout: ChildStdout,
    stderr: Option<ChildStderr>,
    session_id: Arc<Mutex<Option<String>>>,
    chats: Arc<Mutex<HashMap<String, Chat>>>,
    generation: u64,
) {
    if let Some(stderr) = stderr {
        let (app3, id3) = (app.clone(), chat_id.to_string());
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = app3.emit(
                    "claude://event",
                    ChatEvent {
                        chat_id: id3.clone(),
                        event: json!({ "type": "stderr", "text": line }),
                    },
                );
            }
        });
    }
    let app2 = app.clone();
    let id2 = chat_id.to_string();
    let root2 = root.to_path_buf();
    std::thread::spawn(move || {
        let mut decoder = crate::assistants::AgyDecoder::default();
        let mut reader = BufReader::new(stdout);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(_) => break,
            }
            if buf.last() == Some(&b'\n') {
                buf.pop();
            }
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
            if buf.is_empty() {
                continue;
            }
            // One bad byte used to end the whole chat, so the reply never arrived.
            let line = String::from_utf8_lossy(&buf);
            let Ok(event) = serde_json::from_str::<Value>(line.as_ref()) else {
                continue;
            };
            if let Some(sid) = event
                .get("conversation_id")
                .and_then(Value::as_str)
                .or_else(|| {
                    event
                        .pointer("/result/conversation_id")
                        .and_then(Value::as_str)
                })
            {
                *session_id.lock().unwrap() = Some(sid.to_string());
                touch_session(&root2, sid, None, "gemini");
            }
            for translated in decoder.push(&event) {
                if let Some(sid) = translated.get("session_id").and_then(Value::as_str) {
                    *session_id.lock().unwrap() = Some(sid.to_string());
                    touch_session(&root2, sid, None, "gemini");
                }
                let _ = app2.emit(
                    "claude://event",
                    ChatEvent {
                        chat_id: id2.clone(),
                        event: translated,
                    },
                );
            }
        }
        let current = chats.lock().unwrap().get(&id2).map(|chat| chat.generation);
        if current == Some(generation) {
            let _ = app2.emit(
                "claude://event",
                ChatEvent {
                    chat_id: id2,
                    event: json!({ "type": "exit" }),
                },
            );
        }
    });
}

/// Give Codex an API key on stdin, or point Antigravity at the Gemini API.
/// The key is not written into a shell command.
pub fn login_api_key(assistant: &str, key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("enter an API key".into());
    }
    match crate::assistants::normalize(Some(assistant)) {
        "chatgpt" => {
            let bin = find_named(crate::assistants::bin_names("chatgpt"))
                .ok_or("Codex is not installed")?;
            let mut child = Command::new(bin)
                .args(["login", "--with-api-key"])
                .env("PATH", child_path())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("could not start Codex: {e}"))?;
            if let Some(mut stdin) = child.stdin.take() {
                writeln!(stdin, "{key}").map_err(|e| e.to_string())?;
            }
            let out = child.wait_with_output().map_err(|e| e.to_string())?;
            if !out.status.success() {
                let mut detail = String::from_utf8_lossy(&out.stderr).replace(key, "[key]");
                detail.push_str(&String::from_utf8_lossy(&out.stdout).replace(key, "[key]"));
                return Err(format!("Codex rejected the API key: {}", detail.trim()));
            }
            Ok(())
        }
        "gemini" => {
            let path = crate::assistants::settings_path().ok_or("no home directory")?;
            crate::assistants::set_model_provider(&path, true)
        }
        _ => Ok(()),
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
            .args([
                "-e",
                &format!("tell application \"Terminal\" to do script \"{escaped}\""),
                "-e",
                "tell application \"Terminal\" to activate",
            ])
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    if cfg!(windows) {
        Command::new("cmd")
            .args([
                "/C",
                "start",
                "",
                "cmd",
                "/K",
                &format!("cd /d {} && {}", shell_quote(&dir_s), command),
            ])
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    let script = format!(
        "cd {} && {}; exec \"${{SHELL:-/bin/sh}}\"",
        shell_quote(&dir_s),
        command
    );
    for (term, flag) in [
        ("x-terminal-emulator", "-e"),
        ("gnome-terminal", "--"),
        ("konsole", "-e"),
        ("xfce4-terminal", "-x"),
        ("kitty", "--"),
        ("alacritty", "-e"),
        ("xterm", "-e"),
    ] {
        if Command::new(term)
            .args([flag, "sh", "-c", &script])
            .env("PATH", child_path())
            .spawn()
            .is_ok()
        {
            return Ok(());
        }
    }
    Err(format!(
        "no terminal emulator found; run this yourself in {dir_s}:\n{command}"
    ))
}

/// Point a Claude Code process at the provider chosen in the sign-in dialog.
/// Cloud-provider variables already in the environment are cleared first so a
/// claude.ai or Console choice is not overridden by a leftover shell export.
pub fn apply_provider(
    cmd: &mut Command,
    provider: &str,
    foundry_resource: Option<&str>,
    foundry_api_key: Option<&str>,
) {
    for key in [
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
    ] {
        cmd.env_remove(key);
    }
    match provider {
        "bedrock" => {
            cmd.env("CLAUDE_CODE_USE_BEDROCK", "1");
        }
        "vertex" => {
            cmd.env("CLAUDE_CODE_USE_VERTEX", "1");
        }
        "foundry" => {
            cmd.env("CLAUDE_CODE_USE_FOUNDRY", "1");
            if let Some(resource) = nonempty(foundry_resource) {
                cmd.env("ANTHROPIC_FOUNDRY_RESOURCE", resource);
            }
            if let Some(key) = nonempty(foundry_api_key) {
                cmd.env("ANTHROPIC_FOUNDRY_API_KEY", key);
            }
        }
        _ => {}
    }
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty())
}

fn quote_shell(s: &str, windows: bool) -> String {
    if windows {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn announce(lines: &[&str], windows: bool) -> String {
    if windows {
        lines
            .iter()
            .map(|line| format!("echo {line}"))
            .collect::<Vec<_>>()
            .join(" && ")
    } else {
        let args = lines
            .iter()
            .map(|line| quote_shell(line, false))
            .collect::<Vec<_>>()
            .join(" ");
        format!("printf '%s\\n' {args} ''")
    }
}

/// Shell command that signs in with the chosen provider.
///
/// The API key is never placed on the command line. The panel process receives
/// it through the environment instead, so it does not land in shell history.
pub fn login_command(
    exe: &str,
    provider: &str,
    foundry_resource: Option<&str>,
    windows: bool,
) -> String {
    let exe = quote_shell(exe, windows);
    let launch = if windows {
        exe.clone()
    } else {
        format!("exec {exe}")
    };
    match provider {
        "console" => format!("{exe} auth login --console"),
        "bedrock" => {
            let note = announce(
                &[
                    "At the login prompt, choose 3rd-party platform, then Amazon Bedrock.",
                    "If you are already signed in, type /login and make the same choice. The wizard asks which models to pin.",
                ],
                windows,
            );
            format!("{note} && {launch}")
        }
        "vertex" => {
            let note = announce(
                &[
                    "At the login prompt, choose 3rd-party platform, then Google Vertex AI.",
                    "If you are already signed in, type /login and make the same choice. Run /setup-vertex later to change the pinned models.",
                ],
                windows,
            );
            format!("{note} && {launch}")
        }
        "foundry" => {
            let resource = nonempty(foundry_resource).unwrap_or("").replace('"', "");
            let enable = if windows {
                "set \"CLAUDE_CODE_USE_FOUNDRY=1\""
            } else {
                "export CLAUDE_CODE_USE_FOUNDRY=1"
            };
            let mut parts = vec![enable.to_string()];
            if !resource.is_empty() {
                if windows {
                    parts.push(format!("set \"ANTHROPIC_FOUNDRY_RESOURCE={resource}\""));
                } else {
                    parts.push(format!(
                        "export ANTHROPIC_FOUNDRY_RESOURCE={}",
                        quote_shell(&resource, false)
                    ));
                }
            }
            parts.push(announce(
                &[
                    "Sign in with Azure if this resource uses Microsoft Entra ID: az login",
                    "An API key entered in the app is used by the Claude panel and is not printed here.",
                ],
                windows,
            ));
            parts.push(launch);
            parts.join(" && ")
        }
        _ => format!("{exe} auth login --claudeai"),
    }
}

fn open_assistant_terminal(
    root: &Path,
    action: &str,
    session: Option<&str>,
    assistant: &str,
) -> Result<String, String> {
    let missing = if assistant == "chatgpt" {
        "Codex is not installed"
    } else {
        "Antigravity is not installed"
    };
    let bin = find_named(crate::assistants::bin_names(assistant)).ok_or(missing)?;
    let exe = shell_quote(&bin.display().to_string());
    let command = match (assistant, action) {
        ("chatgpt", "login") => format!("{exe} login"),
        ("chatgpt", "resume") => match session {
            Some(id) => format!("{exe} resume {}", shell_quote(id)),
            None => exe,
        },
        ("chatgpt", "remote") => {
            return Err("Continuing on another device is part of Claude. ChatGPT chats stay on this computer.".into());
        }
        ("gemini", "login") => {
            if let Some(path) = crate::assistants::settings_path() {
                crate::assistants::set_model_provider(&path, false)?;
            }
            exe
        }
        ("gemini", "resume") => match session {
            Some(id) => format!("{exe} --conversation {}", shell_quote(id)),
            None => exe,
        },
        ("gemini", "remote") => {
            return Err("Continuing on another device is part of Claude. Gemini chats stay on this computer.".into());
        }
        _ => return Err(format!("unknown action {action}")),
    };
    open_terminal(root, &command)?;
    Ok(command)
}

/// `login`: sign in. `remote`: continue a Claude chat from another device.
pub fn open_in_terminal(
    root: &Path,
    action: &str,
    session: Option<&str>,
    title: &str,
    provider: Option<&str>,
    foundry_resource: Option<&str>,
    assistant: Option<&str>,
) -> Result<String, String> {
    match crate::assistants::normalize(assistant) {
        "chatgpt" | "gemini" => {
            return open_assistant_terminal(
                root,
                action,
                session,
                crate::assistants::normalize(assistant),
            );
        }
        _ => {}
    }
    let claude = find_claude().ok_or("Claude Code is not installed")?;
    let exe = shell_quote(&claude.display().to_string());
    let command = match action {
        "login" => login_command(
            &claude.display().to_string(),
            provider.unwrap_or("claudeai"),
            foundry_resource,
            cfg!(windows),
        ),
        "remote" => {
            let mut c = format!(
                "{exe} --mcp-config {} --strict-mcp-config",
                shell_quote(&mcp_config()?)
            );
            if let Some(s) = session {
                c.push_str(&format!(" --resume {}", shell_quote(s)));
            }
            c.push_str(&format!(" --remote-control {}", shell_quote(title)));
            c
        }
        "resume" => {
            let mut c = format!(
                "{exe} --mcp-config {} --strict-mcp-config",
                shell_quote(&mcp_config()?)
            );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_from_auth_status_maps_known_accounts() {
        assert_eq!(provider_from_status("firstParty", "claude.ai"), "claudeai");
        assert_eq!(provider_from_status("firstParty", "console"), "console");
        assert_eq!(provider_from_status("bedrock", "aws"), "bedrock");
        assert_eq!(provider_from_status("vertex", ""), "vertex");
        assert_eq!(provider_from_status("Microsoft Foundry", ""), "foundry");
    }

    #[test]
    fn login_command_selects_the_account_flag() {
        let claudeai = login_command("/usr/bin/claude", "claudeai", None, false);
        let console = login_command("/usr/bin/claude", "console", None, false);
        assert!(claudeai.contains("auth login --claudeai"), "{claudeai}");
        assert!(console.contains("auth login --console"), "{console}");
        assert!(!console.contains("claudeai"), "{console}");
    }

    #[test]
    fn login_command_quotes_a_path_with_spaces() {
        let cmd = login_command("/Users/a b/claude", "claudeai", None, false);
        assert!(cmd.contains("'/Users/a b/claude'"), "{cmd}");
    }

    #[test]
    fn cloud_login_names_the_provider_and_starts_claude() {
        let bedrock = login_command("claude", "bedrock", None, false);
        assert!(bedrock.contains("Amazon Bedrock"), "{bedrock}");
        assert!(bedrock.contains("exec 'claude'"), "{bedrock}");
        let vertex = login_command("claude", "vertex", None, false);
        assert!(vertex.contains("Google Vertex AI"), "{vertex}");
        let foundry = login_command("claude", "foundry", Some("my-resource"), false);
        assert!(foundry.contains("CLAUDE_CODE_USE_FOUNDRY=1"), "{foundry}");
        assert!(
            foundry.contains("ANTHROPIC_FOUNDRY_RESOURCE='my-resource'"),
            "{foundry}"
        );
        assert!(!foundry.contains("API_KEY"), "{foundry}");
    }

    #[test]
    fn windows_login_does_not_use_exec() {
        let cmd = login_command(r"C:\Program Files\claude.exe", "bedrock", None, true);
        assert!(cmd.contains("Amazon Bedrock"), "{cmd}");
        assert!(!cmd.contains("exec "), "{cmd}");
        assert!(cmd.contains("\"C:\\Program Files\\claude.exe\""), "{cmd}");
    }

    fn reply(behavior: &str) -> PermissionReply {
        PermissionReply {
            request_id: "req-1".into(),
            behavior: behavior.into(),
            message: String::new(),
            tool_use_id: Some("toolu_01".into()),
            updated_input: None,
            updated_permissions: None,
            decision: None,
            allow_rule: None,
        }
    }

    #[test]
    fn allow_once_is_the_control_response_the_cli_reads() {
        let v = control_response(&reply("allow")).unwrap();
        assert_eq!(v["type"], "control_response");
        assert_eq!(v["response"]["subtype"], "success");
        assert_eq!(v["response"]["request_id"], "req-1");
        let body = &v["response"]["response"];
        assert_eq!(body["behavior"], "allow");
        assert_eq!(body["decisionClassification"], "user_temporary");
        assert_eq!(body["toolUseID"], "toolu_01");
        // Missing updatedInput makes the CLI throw inside Object.keys.
        assert!(body["updatedInput"].is_object());
    }

    #[test]
    fn deny_always_carries_a_message() {
        let v = control_response(&reply("deny")).unwrap();
        let body = &v["response"]["response"];
        assert_eq!(body["behavior"], "deny");
        assert_eq!(body["decisionClassification"], "user_reject");
        assert!(body["message"].as_str().unwrap().contains("denied"));
    }

    #[test]
    fn always_allow_hands_the_cli_its_own_rule() {
        let mut reply = reply("allow");
        reply.decision = Some("user_permanent".into());
        reply.updated_permissions = Some(json!([
            { "type": "addRules", "rules": [{ "toolName": "WebSearch" }], "behavior": "allow", "destination": "session" }
        ]));
        let body = &control_response(&reply).unwrap()["response"]["response"];
        assert_eq!(body["decisionClassification"], "user_permanent");
        assert_eq!(
            body["updatedPermissions"][0]["rules"][0]["toolName"],
            "WebSearch"
        );
    }

    #[test]
    fn an_unknown_control_request_is_refused_rather_than_dropped() {
        let v = refuse_control("r-9", "something_invented_later");
        assert_eq!(v["type"], "control_response");
        assert_eq!(v["response"]["subtype"], "error");
        assert_eq!(v["response"]["request_id"], "r-9");
        assert!(v["response"]["error"]
            .as_str()
            .unwrap()
            .contains("something_invented_later"));
    }

    fn attachment(name: &str, media: &str, bytes: &[u8]) -> ChatAttachment {
        ChatAttachment {
            name: name.into(),
            media_type: media.into(),
            data: B64.encode(bytes),
        }
    }

    #[test]
    fn slash_commands_stay_at_the_start_and_images_become_blocks() {
        let dir = std::env::temp_dir().join(format!("lwp-attach-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(".gitignore"), ".paper/owner.lock\n").unwrap();
        let png = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a];
        let files = prepare_attachments(
            &dir,
            &[
                attachment("plot.png", "image/png", &png),
                attachment("notes.tex", "text/plain", br"\section{A}"),
                attachment("scan.pdf", "application/pdf", b"%PDF-1.4"),
            ],
        )
        .unwrap();
        let ctx = EditorContext {
            path: Some("main.tex".into()),
            line: Some(4),
            section: None,
            selection: None,
            mentions: vec![],
        };
        let line = outbound_line("claude", "What is this figure?", &ctx, &files);
        let blocks = &line["message"]["content"];
        let text = blocks[0]["text"].as_str().unwrap();
        assert!(text.contains("<editor_context>"), "{text}");
        assert!(text.contains("notes.tex"), "{text}");
        assert!(text.contains("\\section{A}"), "{text}");
        assert_eq!(blocks[1]["type"], "image");
        assert_eq!(blocks[1]["source"]["media_type"], "image/png");
        assert_eq!(
            B64.decode(blocks[1]["source"]["data"].as_str().unwrap())
                .unwrap(),
            png
        );
        assert_eq!(blocks[2]["type"], "document");
        assert_eq!(blocks[2]["source"]["media_type"], "application/pdf");
        let slash = outbound_line("claude", "/usage", &ctx, &files);
        let command = slash["message"]["content"][0]["text"].as_str().unwrap();
        assert_eq!(command, "/usage");
        assert_eq!(slash["message"]["content"].as_array().unwrap().len(), 1);
        let gemini = outbound_line("gemini", "look", &EditorContext::default(), &files);
        let body = gemini["message"]["content"].as_str().unwrap();
        assert!(body.contains(".paper/attachments/"), "{body}");
        assert!(!body.contains(&files[0].data), "{body}");
        assert!(slash_for_assistant("gemini", "/status").is_err());
        assert!(slash_for_assistant("claude", "/status").is_ok());
        assert!(!is_slash_command("please /usage later"));
        assert_eq!(slash_name("/usage"), Some("usage"));
        assert_eq!(slash_name("/plugin list"), Some("plugin"));
        assert!(slash_name("rewrite this section").is_none());
        let tagged = message_text(
            "Tighten this.",
            &EditorContext {
                mentions: vec![
                    ContextMention {
                        kind: "section".into(),
                        path: Some("main.tex".into()),
                        section: Some("Introduction".into()),
                        line: Some(12),
                        text: Some("prior work".into()),
                    },
                    ContextMention {
                        kind: "file".into(),
                        path: Some("refs.bib".into()),
                        section: None,
                        line: None,
                        text: None,
                    },
                ],
                ..EditorContext::default()
            },
            None,
        );
        assert!(
            tagged.contains("section \"Introduction\" in main.tex near line 12"),
            "{tagged}"
        );
        assert!(tagged.contains("prior work"), "{tagged}");
        assert!(tagged.contains("file refs.bib"), "{tagged}");
        assert!(tagged.contains("Tighten this."), "{tagged}");
        prepare_attachments(&dir, &[attachment("again.txt", "text/plain", b"ok")]).unwrap();
        let ignore = fs::read_to_string(dir.join(".gitignore")).unwrap();
        assert_eq!(ignore.matches(".paper/attachments/").count(), 1, "{ignore}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn plan_usage_lists_session_weekly_and_extra_spend() {
        let body = json!({
            "limits": [
                {"kind": "session", "percent": 11, "resets_at": "2026-10-26T21:29:59.658450+00:00"},
                {"kind": "weekly_all", "percent": 9, "resets_at": "2026-11-02T01:59:59+00:00"}
            ],
            "extra_usage": {
                "is_enabled": true,
                "monthly_limit": 5000,
                "used_credits": 4138.0,
                "utilization": 82.76,
                "currency": "USD",
                "decimal_places": 2
            }
        });
        let text = format_plan_usage(&body, "team");
        assert!(text.contains("Team"), "{text}");
        assert!(
            text.contains("Session limit: 11% used, resets 2026-10-26 21:29 UTC"),
            "{text}"
        );
        assert!(text.contains("Weekly limit: 9% used"), "{text}");
        assert!(
            text.contains("Extra usage: $41.38 of $50.00, 82.8%"),
            "{text}"
        );
        assert!(!text.contains("accessToken"), "{text}");
    }

    #[test]
    fn an_oversized_attachment_is_refused() {
        let dir = std::env::temp_dir().join(format!("lwp-attach-big-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let err = prepare_attachments(
            &dir,
            &[attachment(
                "big.txt",
                "text/plain",
                &vec![b'a'; MAX_ATTACHMENT_BYTES + 1],
            )],
        )
        .unwrap_err();
        assert!(err.contains("4 MB"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }
}
