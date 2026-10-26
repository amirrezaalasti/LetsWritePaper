//! ChatGPT (Codex CLI) and Gemini (Antigravity CLI) beside Claude Code.
//!
//! Both speak their own JSON lines. This module turns those lines into the
//! event shape the assistant panel already renders for Claude, and builds the
//! argv used to start each CLI with this paper's MCP server.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

pub fn normalize(assistant: Option<&str>) -> &'static str {
    match assistant.unwrap_or("claude") {
        "chatgpt" | "codex" => "chatgpt",
        "gemini" | "agy" | "antigravity" => "gemini",
        _ => "claude",
    }
}

pub fn bin_names(assistant: &str) -> &'static [&'static str] {
    if cfg!(windows) {
        match normalize(Some(assistant)) {
            "chatgpt" => &["codex.exe", "codex.cmd", "codex"],
            "gemini" => &["agy.exe", "agy.cmd", "agy"],
            _ => &["claude.exe", "claude.cmd", "claude"],
        }
    } else {
        match normalize(Some(assistant)) {
            "chatgpt" => &["codex"],
            "gemini" => &["agy"],
            _ => &["claude"],
        }
    }
}

/// Shell command shown for Claude and Codex, and kept on the status payload as
/// the upstream one-liner. The panel does not run it for Antigravity; see
/// [`install_antigravity`].
pub fn install_command(assistant: &str) -> String {
    match (normalize(Some(assistant)), cfg!(windows)) {
        ("chatgpt", true) => "irm https://chatgpt.com/codex/install.ps1 | iex".into(),
        ("chatgpt", false) => "curl -fsSL https://chatgpt.com/codex/install.sh | sh".into(),
        ("gemini", true) => "irm https://antigravity.google/cli/install.ps1 | iex".into(),
        ("gemini", false) => "curl -fsSL https://antigravity.google/cli/install.sh | bash".into(),
        (_, true) => "irm https://claude.ai/install.ps1 | iex".into(),
        (_, false) => "curl -fsSL https://claude.ai/install.sh | bash".into(),
    }
}

/// `codex exec` argv, including a one-off paper MCP server. The prompt is read
/// from stdin (`-`), so it never has to be quoted. An API key is never placed
/// here: Codex keeps it from `codex login --with-api-key`.
pub fn codex_args(
    app_exe: &str,
    resume: Option<&str>,
    mode: &str,
    model: Option<&str>,
    images: &[String],
) -> Vec<String> {
    let mut args = vec![
        "exec".into(),
        "--json".into(),
        "--skip-git-repo-check".into(),
        "--color".into(),
        "never".into(),
    ];
    if mode == "edits" {
        // Auto-approves workspace edits. It sets the sandbox itself, so it is
        // not combined with `--sandbox`.
        args.push("--approve-for-me".into());
    } else {
        args.push("--sandbox".into());
        args.push("read-only".into());
    }
    if let Some(model) = model.map(str::trim).filter(|m| !m.is_empty()) {
        args.push("-m".into());
        args.push(model.to_string());
    }
    args.push("-c".into());
    args.push(format!("mcp_servers.paper.command={}", toml_basic(app_exe)));
    args.push("-c".into());
    args.push(format!(
        "mcp_servers.paper.args=[{},{},{}]",
        toml_basic("mcp"),
        toml_basic("--agent"),
        toml_basic("codex")
    ));
    args.push("-c".into());
    args.push("mcp_servers.paper.required=true".into());
    if let Some(id) = resume.map(str::trim).filter(|id| !id.is_empty()) {
        args.push("resume".into());
        args.push(id.to_string());
    }
    for image in images {
        let image = image.trim();
        if image.is_empty() {
            continue;
        }
        args.push("--image".into());
        args.push(image.to_string());
    }
    args.push("-".into());
    args
}

/// Antigravity's persistent stdin session. Headless mode cannot ask, so tool
/// calls are pre-approved; otherwise the paper server would be soft-denied.
pub fn agy_args(resume: Option<&str>, model: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "--input-format".into(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--dangerously-skip-permissions".into(),
        "--print-timeout".into(),
        "30m".into(),
    ];
    if let Some(id) = resume.map(str::trim).filter(|id| !id.is_empty()) {
        args.push("--conversation".into());
        args.push(id.to_string());
    }
    if let Some(model) = model.map(str::trim).filter(|m| !m.is_empty()) {
        args.push("--model".into());
        args.push(model.to_string());
    }
    args
}

fn toml_basic(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Merge the paper server into the workspace MCP file Antigravity reads,
/// leaving every other server in place.
pub fn ensure_workspace_mcp(root: &Path, command: &str, agent: &str) -> Result<(), String> {
    let dir = root.join(".agents");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("mcp_config.json");
    let mut value = match std::fs::read_to_string(&path) {
        Ok(text) if text.trim().is_empty() => json!({}),
        Ok(text) => {
            serde_json::from_str(&text).map_err(|e| format!(".agents/mcp_config.json: {e}"))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(e.to_string()),
    };
    let obj = value
        .as_object_mut()
        .ok_or(".agents/mcp_config.json must be a JSON object")?;
    let servers = obj.entry("mcpServers").or_insert_with(|| json!({}));
    let servers = servers
        .as_object_mut()
        .ok_or(".agents/mcp_config.json mcpServers must be an object")?;
    servers.insert(
        "paper".into(),
        json!({ "command": command, "args": ["mcp", "--agent", agent] }),
    );
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// Antigravity uses a signed-in Google account unless `modelProvider` is
/// `gemini`, which sends requests to the Gemini API instead. The API key
/// itself stays in the process environment.
pub fn set_model_provider(path: &Path, use_api_key: bool) -> Result<(), String> {
    if !use_api_key && !path.exists() {
        return Ok(());
    }
    let mut value = match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => json!({}),
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(e.to_string()),
    };
    let obj = value
        .as_object_mut()
        .ok_or("Antigravity settings must be a JSON object")?;
    if use_api_key {
        obj.insert("modelProvider".into(), json!("gemini"));
    } else if obj.get("modelProvider").and_then(Value::as_str) == Some("gemini") {
        obj.remove("modelProvider");
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

pub fn model_provider_is_api(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| {
            v.get("modelProvider")
                .and_then(Value::as_str)
                .map(|s| s == "gemini")
        })
        .unwrap_or(false)
}

pub fn settings_path() -> Option<std::path::PathBuf> {
    crate::claude::home_dir().map(|home| {
        home.join(".gemini")
            .join("antigravity-cli")
            .join("settings.json")
    })
}

// ---- installing Antigravity -------------------------------------------------------

const AGY_MANIFEST_BASE: &str =
    "https://antigravity-cli-auto-updater-974169037036.us-central1.run.app/manifests";
const AGY_DOWNLOAD_ATTEMPTS: u32 = 3;

/// Download the current Antigravity CLI, check the checksum published in its
/// release manifest, and install it where the app already looks (`~/.local/bin`
/// on macOS and Linux, `%LOCALAPPDATA%\agy\bin` on Windows).
///
/// The official `install.sh` always stages the archive at one path,
/// `~/.cache/antigravity/staging/agy.tar.gz`, and deletes that path when the
/// script exits. Its checksum step ignores a failed `shasum`, so a missing
/// archive — a second install cleaning up, or a download that never landed —
/// is reported as "the file may be corrupted or compromised." This downloads
/// into a private temporary file and only treats a real hash mismatch as a
/// checksum failure. A second install waits for the first instead of sharing
/// that file.
pub fn install_antigravity(mut log: impl FnMut(&str)) -> Result<(), String> {
    let _guard = agy_install_lock();
    let dest = agy_install_path()?;
    if installed_binary(&dest) {
        log(&format!(
            "Antigravity is already installed at {}",
            dest.display()
        ));
        return Ok(());
    }
    log("Detecting system…");
    let platform = current_agy_platform()?;
    log(&format!("Platform: {platform}"));
    let temp = agy_temp_dir()?;
    let _cleanup = RemoveDir(temp.clone());
    log("Fetching the release manifest…");
    let manifest = download_text(&manifest_url(&platform), &temp.join("manifest.json"))?;
    let (version, url, sha512) = parse_release(&manifest)?;
    log(&format!("Latest version: {version}"));
    let payload = temp.join(if is_tar_gz(&url) {
        "agy.tar.gz"
    } else {
        "agy.download"
    });
    log(&format!("Downloading Antigravity {version}…"));
    download_verified(&url, &sha512, &payload, &mut log)?;
    log("Checksum verified.");
    let binary = if is_tar_gz(&url) {
        extract_binary(&payload, &temp)?
    } else {
        payload
    };
    log(&format!("Installing to {}", dest.display()));
    install_executable(&binary, &dest)?;
    log("Configuring the shell…");
    configure_shell(&dest, &mut log);
    log(&format!("Installed {}", dest.display()));
    Ok(())
}

fn agy_install_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `os` is `darwin`, `linux`, `android`, or `windows`. `arch` is `x86_64`,
/// `amd64`, `aarch64`, or `arm64`.
pub fn agy_platform(os: &str, arch: &str, musl: bool) -> Result<String, String> {
    let arch_key = arch.to_ascii_lowercase();
    let arch = match arch_key.as_str() {
        "x86_64" | "amd64" => "amd64",
        "aarch64" | "arm64" => "arm64",
        _ => return Err(format!("Antigravity does not publish a build for {arch}")),
    };
    let name = match os {
        "darwin" | "macos" => format!("darwin_{arch}"),
        "windows" => format!("windows_{arch}"),
        "android" => format!("android_{arch}"),
        "linux" if musl => format!("linux_{arch}_musl"),
        "linux" => format!("linux_{arch}"),
        other => return Err(format!("Antigravity does not publish a build for {other}")),
    };
    Ok(name)
}

fn current_agy_platform() -> Result<String, String> {
    let os = if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "android") {
        "android"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        return Err("Antigravity does not publish a build for this operating system".into());
    };
    let arch = if cfg!(target_arch = "x86_64") {
        "amd64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        return Err("Antigravity does not publish a build for this processor".into());
    };
    agy_platform(os, arch, cfg!(target_env = "musl"))
}

fn manifest_url(platform: &str) -> String {
    format!("{AGY_MANIFEST_BASE}/{platform}.json")
}

pub fn agy_binary_path(home: &Path, local_app_data: Option<&Path>, windows: bool) -> PathBuf {
    if windows {
        local_app_data
            .unwrap_or(home)
            .join("agy")
            .join("bin")
            .join("agy.exe")
    } else {
        home.join(".local").join("bin").join("agy")
    }
}

fn agy_install_path() -> Result<PathBuf, String> {
    if cfg!(windows) {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("USERPROFILE")
                    .map(|home| PathBuf::from(home).join("AppData").join("Local"))
            })
            .ok_or("could not find the local application data folder")?;
        Ok(agy_binary_path(Path::new(""), Some(&base), true))
    } else {
        let home = crate::claude::home_dir().ok_or("could not find the home directory")?;
        Ok(agy_binary_path(&home, None, false))
    }
}

fn installed_binary(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.len() > 1024 * 1024)
        .unwrap_or(false)
}

fn parse_release(text: &str) -> Result<(String, String, String), String> {
    let value: Value = serde_json::from_str(text)
        .map_err(|e| format!("the release manifest was not valid JSON: {e}"))?;
    let field = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let version = field("version");
    let url = field("url");
    let sha512 = field("sha512");
    if version.is_empty() || url.is_empty() || sha512.is_empty() {
        return Err("the release manifest is missing a version, URL, or checksum".into());
    }
    require_https(&url)?;
    Ok((version, url, sha512))
}

fn require_https(url: &str) -> Result<(), String> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err("the release URL is not an https address".into())
    }
}

pub fn is_tar_gz(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.ends_with(".tar.gz") || path.ends_with(".tgz")
}

pub fn checksum_matches(actual: &str, expected: &str) -> bool {
    let actual = actual.trim();
    let expected = expected.trim();
    !actual.is_empty() && actual.eq_ignore_ascii_case(expected)
}

fn download_verified(
    url: &str,
    expected: &str,
    dest: &Path,
    log: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let mut last = "could not download Antigravity".to_string();
    for attempt in 1..=AGY_DOWNLOAD_ATTEMPTS {
        if let Err(err) = curl_to(url, dest) {
            last = err;
            let _ = std::fs::remove_file(dest);
            if attempt < AGY_DOWNLOAD_ATTEMPTS {
                log("The download did not finish. Trying again…");
            }
            continue;
        }
        match sha512_file(dest) {
            Ok(actual) if checksum_matches(&actual, expected) => return Ok(()),
            Ok(_) => {
                last = "the download did not match the published checksum".into();
                let _ = std::fs::remove_file(dest);
                if attempt < AGY_DOWNLOAD_ATTEMPTS {
                    log("The download did not match the published checksum. Trying again…");
                }
            }
            Err(err) => {
                last = err;
                let _ = std::fs::remove_file(dest);
                if attempt < AGY_DOWNLOAD_ATTEMPTS {
                    log("The download could not be checked. Trying again…");
                }
            }
        }
    }
    Err(last)
}

fn download_text(url: &str, dest: &Path) -> Result<String, String> {
    let mut last = "could not download the release manifest".to_string();
    for _ in 1..=AGY_DOWNLOAD_ATTEMPTS {
        match curl_to(url, dest) {
            Ok(()) => {
                return std::fs::read_to_string(dest)
                    .map_err(|e| format!("could not read the release manifest: {e}"));
            }
            Err(err) => {
                last = err;
                let _ = std::fs::remove_file(dest);
            }
        }
    }
    Err(last)
}

fn curl_to(url: &str, dest: &Path) -> Result<(), String> {
    require_https(url)?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    let output = Command::new("curl")
        .args([
            "--proto",
            "=https",
            "--tlsv1.2",
            "-fsSL",
            "--connect-timeout",
            "20",
            "--max-time",
            "600",
            "-o",
        ])
        .arg(dest)
        .arg(url)
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "curl is required to download Antigravity".to_string()
            } else {
                format!("could not start curl: {e}")
            }
        })?;
    if output.status.success() && dest.is_file() {
        return Ok(());
    }
    let _ = std::fs::remove_file(dest);
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("download failed");
    Err(format!("could not download {url}: {detail}"))
}

fn sha512_file(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha512};
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("could not read the downloaded package: {e}"))?;
    let mut hasher = Sha512::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("could not read the downloaded package: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_encode(hasher.finalize()))
}

fn hex_encode(bytes: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = bytes.as_ref();
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

fn extract_binary(archive: &Path, dir: &Path) -> Result<PathBuf, String> {
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(dir)
        .arg("antigravity")
        .status()
        .map_err(|e| format!("could not extract Antigravity: {e}"))?;
    if !status.success() {
        return Err("could not extract the Antigravity program from the archive".into());
    }
    let binary = dir.join("antigravity");
    if !binary.is_file() {
        return Err("the Antigravity archive did not contain the program".into());
    }
    Ok(binary)
}

fn install_executable(src: &Path, dest: &Path) -> Result<(), String> {
    let parent = dest
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or("the Antigravity install path has no directory")?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    let mut temporary_name = dest.file_name().unwrap_or_default().to_os_string();
    temporary_name.push(".download");
    let temporary = parent.join(temporary_name);
    std::fs::copy(src, &temporary)
        .map_err(|e| format!("could not write {}: {e}", temporary.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&temporary)
            .map_err(|e| e.to_string())?
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&temporary, permissions)
            .map_err(|e| format!("could not mark Antigravity as executable: {e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("xattr")
            .args(["-d", "com.apple.quarantine"])
            .arg(&temporary)
            .status();
    }
    if dest.exists() {
        std::fs::remove_file(dest)
            .map_err(|e| format!("could not replace {}: {e}", dest.display()))?;
    }
    if std::fs::rename(&temporary, dest).is_err() {
        std::fs::copy(&temporary, dest)
            .map_err(|e| format!("could not install {}: {e}", dest.display()))?;
        let _ = std::fs::remove_file(&temporary);
    }
    Ok(())
}

fn configure_shell(binary: &Path, log: &mut dyn FnMut(&str)) {
    let output = Command::new(binary)
        .arg("install")
        .stdin(Stdio::null())
        .output();
    let output = match output {
        Ok(output) => output,
        Err(err) => {
            log(&format!(
                "Could not finish shell setup ({err}). Antigravity is installed at {}.",
                binary.display()
            ));
            return;
        }
    };
    let mut lines = String::from_utf8_lossy(&output.stdout).into_owned();
    lines.push('\n');
    for line in String::from_utf8_lossy(&output.stderr).lines() {
        if line.contains("logging before google.Init") {
            continue;
        }
        lines.push_str(line);
        lines.push('\n');
    }
    for line in lines.lines() {
        let line = line.trim();
        if !line.is_empty() {
            log(line);
        }
    }
    if !output.status.success() {
        log(&format!(
            "Shell setup reported a problem. Antigravity is installed at {}.",
            binary.display()
        ));
    }
}

fn agy_temp_dir() -> Result<PathBuf, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("lwp-agy-{}-{millis}", std::process::id()));
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create a temporary folder: {e}"))?;
    Ok(dir)
}

struct RemoveDir(PathBuf);

impl Drop for RemoveDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Codex JSONL events, as the panel's Claude-shaped stream.
pub fn translate_codex(event: &Value, model: &str) -> Vec<Value> {
    match event.get("type").and_then(Value::as_str).unwrap_or("") {
        "thread.started" => {
            let id = event.get("thread_id").and_then(Value::as_str).unwrap_or("");
            vec![json!({
                "type": "system",
                "subtype": "init",
                "session_id": id,
                "model": if model.is_empty() { "ChatGPT" } else { model },
            })]
        }
        "item.completed" => translate_codex_item(event.get("item").unwrap_or(&Value::Null)),
        "turn.completed" => vec![json!({ "type": "result", "is_error": false, "num_turns": 1 })],
        "turn.failed" | "error" => {
            let message = event
                .pointer("/error/message")
                .and_then(Value::as_str)
                .or_else(|| event.get("message").and_then(Value::as_str))
                .unwrap_or("ChatGPT stopped with an error");
            vec![json!({ "type": "result", "is_error": true, "result": message })]
        }
        _ => vec![],
    }
}

fn translate_codex_item(item: &Value) -> Vec<Value> {
    let id = item.get("id").and_then(Value::as_str).unwrap_or("item");
    match item.get("type").and_then(Value::as_str).unwrap_or("") {
        "agent_message" => {
            let text = item.get("text").and_then(Value::as_str).unwrap_or("");
            if text.is_empty() {
                vec![]
            } else {
                vec![json!({
                    "type": "assistant",
                    "message": { "id": id, "content": [{ "type": "text", "text": text }] }
                })]
            }
        }
        "mcp_tool_call" => {
            let name = codex_tool_name(item);
            let input = item
                .get("arguments")
                .or_else(|| item.get("input"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            let (output, is_error) = codex_tool_output(item);
            tool_pair(id, &name, input, &output, is_error)
        }
        "command_execution" => {
            let command = item
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("command");
            let output = item
                .get("aggregated_output")
                .and_then(Value::as_str)
                .unwrap_or("");
            let is_error = item.get("exit_code").and_then(Value::as_i64).unwrap_or(0) != 0;
            tool_pair(
                id,
                "command",
                json!({ "command": command }),
                output,
                is_error,
            )
        }
        "file_change" => tool_pair(
            id,
            "file_change",
            item.get("changes").cloned().unwrap_or_else(|| json!([])),
            "",
            false,
        ),
        _ => vec![],
    }
}

fn codex_tool_name(item: &Value) -> String {
    let tool = item
        .get("tool")
        .or_else(|| item.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("tool");
    let server = item.get("server").and_then(Value::as_str).unwrap_or("");
    if server == "paper" {
        format!("mcp__paper__{tool}")
    } else if tool.starts_with("mcp__") {
        tool.to_string()
    } else {
        paper_tool_name(tool)
    }
}

fn codex_tool_output(item: &Value) -> (String, bool) {
    if let Some(error) = item.get("error").filter(|v| !v.is_null()) {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("tool failed");
        return (message.to_string(), true);
    }
    let text = item
        .get("result")
        .or_else(|| item.get("output"))
        .map(value_text)
        .unwrap_or_default();
    (text, false)
}

fn paper_tool_name(name: &str) -> String {
    const TOOLS: &[&str] = &[
        "project_overview",
        "read_document",
        "outline",
        "search",
        "section_context",
        "edit_document",
        "write_document",
        "propose_edit",
        "list_proposals",
        "history",
        "diff",
        "blame",
        "checkpoint",
        "compile",
        "list_skills",
        "get_skill",
        "bind_skill",
        "create_skill",
        "start_review",
        "add_review_comment",
        "finish_review",
        "list_review_comments",
        "respond_to_comment",
    ];
    if TOOLS.contains(&name) {
        format!("mcp__paper__{name}")
    } else {
        name.to_string()
    }
}

fn tool_pair(id: &str, name: &str, input: Value, output: &str, is_error: bool) -> Vec<Value> {
    vec![
        json!({
            "type": "assistant",
            "message": {
                "id": format!("m-{id}"),
                "content": [{ "type": "tool_use", "id": id, "name": name, "input": input }]
            }
        }),
        json!({
            "type": "user",
            "message": {
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": id,
                    "is_error": is_error,
                    "content": output,
                }]
            }
        }),
    ]
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Antigravity's `stream-json` session, folded into the same event shape.
#[derive(Default)]
pub struct AgyDecoder {
    step: Option<i64>,
    saw_delta: bool,
}

impl AgyDecoder {
    pub fn push(&mut self, event: &Value) -> Vec<Value> {
        match event.get("event").and_then(Value::as_str).unwrap_or("") {
            "init" => {
                let id = event
                    .get("conversation_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let model = event
                    .pointer("/init/model")
                    .and_then(Value::as_str)
                    .unwrap_or("Gemini");
                vec![json!({
                    "type": "system",
                    "subtype": "init",
                    "session_id": id,
                    "model": model,
                })]
            }
            "step_update" => self.on_step(event.get("step_update").unwrap_or(&Value::Null)),
            "result" => {
                let result = event.get("result").cloned().unwrap_or_else(|| json!({}));
                let status = result
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("SUCCESS");
                let is_error = status != "SUCCESS";
                let mut ev = json!({
                    "type": "result",
                    "is_error": is_error,
                    "num_turns": result.get("num_turns").cloned().unwrap_or(json!(1)),
                });
                if let Some(seconds) = result.get("duration_seconds").and_then(Value::as_f64) {
                    ev["duration_ms"] = json!((seconds * 1000.0).round() as u64);
                }
                if is_error {
                    let message = result
                        .get("error")
                        .map(value_text)
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "Gemini stopped with an error".into());
                    ev["result"] = json!(message);
                }
                if let Some(id) = result.get("conversation_id").and_then(Value::as_str) {
                    ev["session_id"] = json!(id);
                }
                vec![ev]
            }
            _ => vec![],
        }
    }

    fn on_step(&mut self, step: &Value) -> Vec<Value> {
        let index = step.get("step_index").and_then(Value::as_i64);
        let kind = step.get("step_type").and_then(Value::as_str).unwrap_or("");
        let state = step.get("state").and_then(Value::as_str).unwrap_or("");
        if kind == "agent_response" {
            let mut out = Vec::new();
            if self.step != index {
                self.step = index;
                self.saw_delta = false;
                out.push(json!({
                    "type": "stream_event",
                    "event": {
                        "type": "message_start",
                        "message": { "id": format!("agy-{}", index.unwrap_or(0)) }
                    }
                }));
            }
            if let Some(delta) = step
                .get("text_delta")
                .and_then(Value::as_str)
                .filter(|d| !d.is_empty())
            {
                if !(state == "DONE" && self.saw_delta) {
                    self.saw_delta = true;
                    out.push(json!({
                        "type": "stream_event",
                        "event": { "type": "content_block_delta", "delta": { "type": "text_delta", "text": delta } }
                    }));
                }
            }
            return out;
        }
        if kind == "tool" && state == "DONE" {
            let info = step.get("tool_info").cloned().unwrap_or_else(|| json!({}));
            let raw = step
                .get("tool_name")
                .or_else(|| info.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let name = paper_tool_name(raw);
            let input = info.get("parameters").cloned().unwrap_or_else(|| json!({}));
            let is_error = info.get("error").is_some_and(|e| !e.is_null());
            let output = if is_error {
                info.pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("tool failed")
                    .to_string()
            } else {
                info.get("output").map(value_text).unwrap_or_default()
            };
            return tool_pair(
                &format!("tool-{}", index.unwrap_or(0)),
                &name,
                input,
                &output,
                is_error,
            );
        }
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    #[test]
    fn codex_args_attach_the_paper_server_and_hide_keys() {
        let args = codex_args(
            "/Apps/Lets Write.app/bin",
            Some("thread-1"),
            "paper",
            Some("gpt-6.1-sol"),
            &[],
        );
        let joined = args.join(" ");
        assert!(
            joined.contains("mcp_servers.paper.command=\"/Apps/Lets Write.app/bin\""),
            "{joined}"
        );
        assert!(joined.contains("--agent"), "{joined}");
        assert!(joined.contains("codex"), "{joined}");
        assert!(joined.contains("resume thread-1"), "{joined}");
        assert!(joined.contains("--sandbox read-only"), "{joined}");
        assert!(!joined.contains("api"), "{joined}");
        assert!(args.last() == Some(&"-".to_string()));
    }

    #[test]
    fn codex_edits_do_not_also_set_a_sandbox() {
        let args = codex_args("app", None, "edits", None, &[]);
        assert!(args.iter().any(|a| a == "--approve-for-me"));
        assert!(!args.iter().any(|a| a == "--sandbox"));
    }

    #[test]
    fn codex_images_are_flags_before_stdin() {
        let images = vec!["/tmp/plot.png".into(), "/tmp/scan.jpg".into()];
        let args = codex_args("app", None, "paper", None, &images);
        let first = args.iter().position(|a| a == "--image").unwrap();
        assert_eq!(args[first + 1], "/tmp/plot.png");
        assert_eq!(args[first + 2], "--image");
        assert_eq!(args[first + 3], "/tmp/scan.jpg");
        assert_eq!(args.last().map(String::as_str), Some("-"));
    }

    #[test]
    fn codex_thread_and_reply_become_panel_events() {
        let started = translate_codex(
            &json!({"type": "thread.started", "thread_id": "t1"}),
            "gpt-6.1-sol",
        );
        assert_eq!(started[0]["session_id"], "t1");
        assert_eq!(started[0]["model"], "gpt-6.1-sol");
        let reply = translate_codex(
            &json!({"type": "item.completed", "item": {"id": "m1", "type": "agent_message", "text": "Done"}}),
            "",
        );
        assert_eq!(reply[0]["message"]["content"][0]["text"], "Done");
        let tool = translate_codex(
            &json!({"type": "item.completed", "item": {"id": "c1", "type": "mcp_tool_call", "server": "paper", "tool": "edit_document", "arguments": {"path": "main.tex"}, "result": "ok"}}),
            "",
        );
        assert_eq!(
            tool[0]["message"]["content"][0]["name"],
            "mcp__paper__edit_document"
        );
        assert_eq!(tool[1]["message"]["content"][0]["content"], "ok");
    }

    #[test]
    fn agy_stream_keeps_one_copy_of_a_short_reply_and_a_tool() {
        let mut dec = AgyDecoder::default();
        let init = dec.push(&json!({"event": "init", "conversation_id": "c1", "init": {"model": "gemini-3.8-flash-high"}}));
        assert_eq!(init[0]["session_id"], "c1");
        let text = dec.push(&json!({"event": "step_update", "step_update": {"step_index": 2, "state": "DONE", "step_type": "agent_response", "text_delta": "apple\n"}}));
        assert_eq!(text.len(), 2);
        assert_eq!(text[1]["event"]["delta"]["text"], "apple\n");
        let again = dec.push(&json!({"event": "step_update", "step_update": {"step_index": 2, "state": "DONE", "step_type": "agent_response", "text_delta": "apple\n"}}));
        assert!(
            again.is_empty(),
            "a repeated done delta is not appended twice"
        );
        let tool = dec.push(&json!({"event": "step_update", "step_update": {"step_index": 4, "state": "DONE", "step_type": "tool", "tool_name": "edit_document", "tool_info": {"name": "edit_document", "parameters": {"path": "a.tex"}, "output": "edited"}}}));
        assert_eq!(
            tool[0]["message"]["content"][0]["name"],
            "mcp__paper__edit_document"
        );
        let result = dec.push(&json!({"event": "result", "result": {"conversation_id": "c1", "status": "ERROR", "error": "sign in", "duration_seconds": 1.2, "num_turns": 1}}));
        assert_eq!(result[0]["is_error"], true);
        assert_eq!(result[0]["result"], "sign in");
        assert_eq!(result[0]["duration_ms"], 1200);
    }

    #[test]
    fn workspace_mcp_keeps_other_servers() {
        let root = std::env::temp_dir().join(format!("lwp-mcp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".agents")).unwrap();
        std::fs::write(
            root.join(".agents/mcp_config.json"),
            r#"{"mcpServers":{"other":{"command":"elsewhere"}}}"#,
        )
        .unwrap();
        ensure_workspace_mcp(&root, "/app", "gemini").unwrap();
        ensure_workspace_mcp(&root, "/app", "gemini").unwrap();
        let value: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join(".agents/mcp_config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(value["mcpServers"]["other"]["command"], "elsewhere");
        assert_eq!(value["mcpServers"]["paper"]["args"][2], "gemini");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn agy_platform_names_match_the_release_manifests() {
        assert_eq!(
            agy_platform("darwin", "arm64", false).unwrap(),
            "darwin_arm64"
        );
        assert_eq!(
            agy_platform("macos", "x86_64", false).unwrap(),
            "darwin_amd64"
        );
        assert_eq!(
            agy_platform("linux", "aarch64", false).unwrap(),
            "linux_arm64"
        );
        assert_eq!(
            agy_platform("linux", "amd64", true).unwrap(),
            "linux_amd64_musl"
        );
        assert_eq!(
            agy_platform("windows", "AMD64", false).unwrap(),
            "windows_amd64"
        );
        assert!(agy_platform("windows", "ppc64", false).is_err());
    }

    #[test]
    fn agy_release_parser_rejects_a_missing_checksum_and_a_non_https_url() {
        let (version, url, sha) = parse_release(
            r#"{"version":"1.4.7","url":"https://storage.googleapis.com/agy.tar.gz","sha512":"AbC"}"#,
        )
        .unwrap();
        assert_eq!(version, "1.4.7");
        assert!(is_tar_gz(&url));
        assert!(!is_tar_gz("https://storage.googleapis.com/agy.exe"));
        assert!(checksum_matches(&sha, "abc"));
        assert!(!checksum_matches("", "abc"));
        assert!(
            parse_release(r#"{"version":"1","url":"https://example.com/a","sha512":""}"#).is_err()
        );
        assert!(
            parse_release(r#"{"version":"1","url":"http://example.com/a","sha512":"abc"}"#)
                .is_err()
        );
    }

    #[test]
    fn agy_install_path_follows_the_official_layout() {
        let home = Path::new("/Users/ada");
        assert_eq!(
            agy_binary_path(home, None, false),
            PathBuf::from("/Users/ada/.local/bin/agy")
        );
        assert_eq!(
            agy_binary_path(home, Some(Path::new("C:/Users/ada/AppData/Local")), true),
            PathBuf::from("C:/Users/ada/AppData/Local/agy/bin/agy.exe")
        );
    }

    #[test]
    fn sha512_file_matches_a_known_digest() {
        let path = std::env::temp_dir().join(format!("lwp-agy-sha-{}", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        let hash = sha512_file(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            hash,
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
        let missing = sha512_file(Path::new("/no/such/agy.tar.gz")).unwrap_err();
        assert!(missing.contains("could not read"), "{missing}");
    }

    /// Hits the network. Run with `--ignored` when changing the downloader.
    #[test]
    #[ignore = "downloads the current Antigravity release"]
    fn downloads_and_checks_the_published_antigravity_package() {
        let dir = std::env::temp_dir().join(format!("lwp-agy-it-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let platform = current_agy_platform().unwrap();
        let manifest = download_text(&manifest_url(&platform), &dir.join("manifest.json")).unwrap();
        let (version, url, sha) = parse_release(&manifest).unwrap();
        assert!(!version.is_empty(), "{manifest}");
        let payload = dir.join("payload");
        download_verified(&url, &sha, &payload, &mut |_| {}).unwrap();
        if is_tar_gz(&url) {
            let binary = extract_binary(&payload, &dir).unwrap();
            assert!(std::fs::metadata(&binary).unwrap().len() > 1024 * 1024);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn api_key_provider_is_removed_for_a_google_account() {
        let path = std::env::temp_dir().join(format!("lwp-agy-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        set_model_provider(&path, true).unwrap();
        assert!(model_provider_is_api(&path));
        set_model_provider(&path, false).unwrap();
        assert!(!model_provider_is_api(&path));
        let _ = std::fs::remove_file(&path);
    }
}
