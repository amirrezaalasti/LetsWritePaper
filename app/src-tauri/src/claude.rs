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
Keep replies short; the paper itself is where your work goes. \
WebSearch, WebFetch and any other tool that is not pre-approved raise an allow prompt in this chat; wait for the user's choice instead of saying you cannot.";

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

struct Chat {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    session_id: Arc<Mutex<Option<String>>>,
    assistant: String,
    codex_model: String,
    codex_mode: String,
    /// The `codex exec` process for the turn in flight, if any.
    codex_run: Arc<Mutex<Option<Child>>>,
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
}

fn write_line(
    chats: &Arc<Mutex<HashMap<String, Chat>>>,
    chat_id: &str,
    line: &str,
) -> Result<(), String> {
    let mut chats = chats.lock().unwrap();
    let chat = chats.get_mut(chat_id).ok_or("this chat is not running")?;
    let stdin = chat.stdin.as_mut().ok_or("this chat is not accepting input")?;
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

#[derive(Deserialize, Default)]
pub struct EditorContext {
    pub path: Option<String>,
    pub line: Option<usize>,
    pub section: Option<String>,
    pub selection: Option<String>,
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
        detail: text.lines().next().unwrap_or("").chars().take(180).collect(),
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
    let command = crate::assistants::install_command(assistant);
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &command]);
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
            "gemini" => return self.start_agy(app, root, chat_id, resume, model, provider, api_key),
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
    pub fn permission(&self, chat_id: &str, reply: &PermissionReply) -> Result<(), String> {
        let line = serde_json::to_string(&control_response(reply)?).map_err(|e| e.to_string())?;
        write_line(&self.chats, chat_id, &line)
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
        model: Option<&str>,
        provider: Option<&str>,
        api_key: Option<&str>,
    ) -> Result<(), String> {
        let agy = find_named(crate::assistants::bin_names("gemini")).ok_or("Antigravity is not installed")?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        crate::assistants::ensure_workspace_mcp(root, &exe.display().to_string(), "gemini")?;
        let use_api_key = provider == Some("apikey");
        if let Some(path) = crate::assistants::settings_path() {
            crate::assistants::set_model_provider(&path, use_api_key)?;
        }
        let mut cmd = Command::new(agy);
        cmd.current_dir(root)
            .env("PATH", child_path())
            .env("PAPER_AGENT", "gemini")
            .args(crate::assistants::agy_args(resume, model));
        if use_api_key {
            if let Some(key) = nonempty(api_key) {
                cmd.env("GEMINI_API_KEY", key);
            }
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
        let session_id = Arc::new(Mutex::new(resume.map(str::to_string)));
        self.chats.lock().unwrap().insert(
            chat_id.to_string(),
            Chat {
                child: Some(child),
                stdin: Some(stdin),
                session_id: session_id.clone(),
                assistant: "gemini".into(),
                codex_model: String::new(),
                codex_mode: String::new(),
                codex_run: Arc::new(Mutex::new(None)),
            },
        );
        watch_output(app, chat_id, root, stdout, stderr, session_id);
        Ok(())
    }

    pub fn send(
        &self,
        app: &AppHandle,
        root: &Path,
        chat_id: &str,
        text: &str,
        ctx: &EditorContext,
    ) -> Result<(), String> {
        let assistant = {
            let chats = self.chats.lock().unwrap();
            chats
                .get(chat_id)
                .map(|chat| chat.assistant.clone())
                .ok_or("this chat is not running")?
        };
        if assistant == "chatgpt" {
            return self.send_codex(app, root, chat_id, text, ctx);
        }
        let content = message_text(text, ctx, assistant == "gemini");
        let line = if assistant == "gemini" {
            json!({ "event": "user", "message": { "content": content } })
        } else {
            json!({
                "type": "user",
                "message": { "role": "user", "content": [{ "type": "text", "text": content }] },
                "parent_tool_use_id": null,
            })
        };
        let mut chats = self.chats.lock().unwrap();
        let chat = chats.get_mut(chat_id).ok_or("this chat is not running")?;
        let stdin = chat.stdin.as_mut().ok_or("this chat is not accepting input")?;
        writeln!(stdin, "{line}").map_err(|e| format!("the assistant is not accepting input: {e}"))?;
        stdin.flush().map_err(|e| e.to_string())?;
        if let Some(sid) = chat.session_id.lock().unwrap().clone() {
            let title: String = text.chars().take(60).collect();
            touch_session(root, &sid, Some(&title), &assistant);
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
        let bin = find_named(crate::assistants::bin_names("chatgpt")).ok_or("Codex is not installed")?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut cmd = Command::new(bin);
        cmd.current_dir(root)
            .env("PATH", child_path())
            .env("PAPER_AGENT", "codex")
            .args(crate::assistants::codex_args(
                &exe.display().to_string(),
                resume.as_deref(),
                &mode,
                Some(&model),
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| format!("could not start Codex: {e}"))?;
        let prompt = message_text(text, ctx, true);
        if let Some(mut stdin) = child.stdin.take() {
            writeln!(stdin, "{prompt}").map_err(|e| e.to_string())?;
        }
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take();
        *run.lock().unwrap() = Some(child);
        let app = app.clone();
        let root = root.to_path_buf();
        let chat_id = chat_id.to_string();
        let title: String = text.chars().take(60).collect();
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

    pub fn stop_all(&self) {
        let ids: Vec<String> = self.chats.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.stop(&id);
        }
    }
}

fn message_text(text: &str, ctx: &EditorContext, instructions: bool) -> String {
    let mut content = String::new();
    if instructions {
        content.push_str(SYSTEM_PROMPT);
        content.push_str("\n\n");
    }
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
    content
}

/// Read an Antigravity stream and emit it as panel events.
fn watch_output(
    app: &AppHandle,
    chat_id: &str,
    root: &Path,
    stdout: std::process::ChildStdout,
    stderr: Option<std::process::ChildStderr>,
    session_id: Arc<Mutex<Option<String>>>,
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
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let Ok(event) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(sid) = event
                .get("conversation_id")
                .and_then(Value::as_str)
                .or_else(|| event.pointer("/result/conversation_id").and_then(Value::as_str))
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
        let _ = app2.emit(
            "claude://event",
            ChatEvent {
                chat_id: id2,
                event: json!({ "type": "exit" }),
            },
        );
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
            let bin = find_named(crate::assistants::bin_names("chatgpt")).ok_or("Codex is not installed")?;
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
            return open_assistant_terminal(root, action, session, crate::assistants::normalize(assistant));
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
        assert_eq!(body["updatedPermissions"][0]["rules"][0]["toolName"], "WebSearch");
    }

    #[test]
    fn an_unknown_control_request_is_refused_rather_than_dropped() {
        let v = refuse_control("r-9", "something_invented_later");
        assert_eq!(v["type"], "control_response");
        assert_eq!(v["response"]["subtype"], "error");
        assert_eq!(v["response"]["request_id"], "r-9");
        assert!(v["response"]["error"].as_str().unwrap().contains("something_invented_later"));
    }
}
