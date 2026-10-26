//! First-launch setup: the name edits are attributed to, and optional installs
//! of tectonic (LaTeX) and, via the Claude panel's installer, Claude Code.
//!
//! Tectonic's official drop script unpacks a single binary into the working
//! directory. This module runs that script in a temporary folder and moves the
//! binary into [`paper_core::user_config::bin_dir`], which the compiler searches
//! even when a desktop launch has a short `PATH`.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use paper_core::user_config::UserConfig;
use paper_core::Author;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

#[derive(Clone, Debug, Serialize)]
pub struct SetupStatus {
    pub name: String,
    pub setup_done: bool,
    /// Typst preview is compiled into the desktop app.
    pub typst: bool,
    /// `tectonic`, `latexmk`, or absent.
    pub latex: Option<String>,
    pub latex_path: Option<String>,
    pub claude_installed: bool,
    pub claude_version: Option<String>,
    pub codex_installed: bool,
    pub codex_version: Option<String>,
    pub gemini_installed: bool,
    pub gemini_version: Option<String>,
    /// Command the tectonic button runs, shown so the download is visible.
    pub tectonic_command: String,
}

pub fn status() -> SetupStatus {
    let config = paper_core::user_config::load();
    let latex = paper_compile::latex_engine().map(str::to_string);
    let latex_path = latex
        .as_deref()
        .and_then(paper_compile::find_tool)
        .map(|p| p.display().to_string());
    let claude = crate::claude::status_for("claude");
    let codex = crate::claude::status_for("chatgpt");
    let gemini = crate::claude::status_for("gemini");
    SetupStatus {
        name: if config.name.trim().is_empty() {
            Author::local_user().name
        } else {
            config.name
        },
        setup_done: config.setup_done,
        typst: paper_compile::typst_embedded(),
        latex,
        latex_path,
        claude_installed: claude.installed,
        claude_version: claude.version,
        codex_installed: codex.installed,
        codex_version: codex.version,
        gemini_installed: gemini.installed,
        gemini_version: gemini.version,
        tectonic_command: tectonic_command(),
    }
}

pub fn save(name: String, setup_done: bool) -> Result<SetupStatus, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("enter the name your edits should be attributed to".into());
    }
    let config = UserConfig {
        name: name.to_string(),
        setup_done,
    };
    paper_core::user_config::save(&config).map_err(|e| e.to_string())?;
    Ok(status())
}

pub fn tectonic_command() -> String {
    if cfg!(windows) {
        "[System.Net.ServicePointManager]::SecurityProtocol = [System.Net.ServicePointManager]::SecurityProtocol -bor 3072; iex ((New-Object System.Net.WebClient).DownloadString('https://drop-ps1.fullyjustified.net'))".into()
    } else {
        "curl --proto '=https' --tlsv1.2 -fsSL https://drop-sh.fullyjustified.net | sh".into()
    }
}

/// Download tectonic with its official installer and keep the binary where the app can find it.
pub fn install_tectonic(app: &AppHandle) -> Result<SetupStatus, String> {
    if !cfg!(windows) {
        let ok = Command::new("curl")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            return Err("curl is required to download tectonic".into());
        }
    }
    let tmp = std::env::temp_dir().join(format!("lwp-tectonic-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)
        .map_err(|e| format!("could not create a temporary folder: {e}"))?;
    let cleanup = tmp.clone();
    let result = (|| {
        emit(app, "Downloading tectonic…");
        let status_code = run_installer(app, &tmp)?;
        if !status_code.success() {
            return Err(format!("the tectonic installer exited with {status_code}"));
        }
        let found = find_downloaded(&tmp, 2)
            .ok_or("the installer finished, but no tectonic program was found")?;
        let dest_dir = paper_core::user_config::bin_dir();
        std::fs::create_dir_all(&dest_dir)
            .map_err(|e| format!("could not create {}: {e}", dest_dir.display()))?;
        let dest = dest_dir.join(found.file_name().unwrap_or_else(|| "tectonic".as_ref()));
        install_file(&found, &dest)?;
        emit(app, format!("Installed {}", dest.display()));
        Ok(status())
    })();
    let _ = std::fs::remove_dir_all(cleanup);
    result
}

fn emit(app: &AppHandle, line: impl AsRef<str>) {
    let _ = app.emit("setup://install-log", line.as_ref().to_string());
}

fn run_installer(app: &AppHandle, dir: &Path) -> Result<std::process::ExitStatus, String> {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("powershell");
        c.args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &tectonic_command(),
        ]);
        c
    } else {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", &tectonic_command()]);
        c
    };
    let mut child = cmd
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start the tectonic installer: {e}"))?;
    let stderr = child.stderr.take();
    let app2 = app.clone();
    let err_thread = std::thread::spawn(move || {
        if let Some(stream) = stderr {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                let _ = app2.emit("setup://install-log", line);
            }
        }
    });
    if let Some(stream) = child.stdout.take() {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            emit(app, line);
        }
    }
    let _ = err_thread.join();
    child.wait().map_err(|e| e.to_string())
}

fn find_downloaded(dir: &Path, depth: usize) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut nested = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_file() && (name == "tectonic" || name == "tectonic.exe") {
            return Some(path);
        }
        if path.is_dir() {
            nested.push(path);
        }
    }
    if depth == 0 {
        return None;
    }
    for path in nested {
        if let Some(found) = find_downloaded(&path, depth - 1) {
            return Some(found);
        }
    }
    None
}

fn install_file(src: &Path, dest: &Path) -> Result<(), String> {
    if dest.exists() {
        std::fs::remove_file(dest)
            .map_err(|e| format!("could not replace {}: {e}", dest.display()))?;
    }
    if std::fs::rename(src, dest).is_err() {
        std::fs::copy(src, dest)
            .map_err(|e| format!("could not copy tectonic to {}: {e}", dest.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(dest)
            .map_err(|e| e.to_string())?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(dest, perms)
            .map_err(|e| format!("could not mark tectonic as executable: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_dropped_tectonic_binary() {
        let dir = std::env::temp_dir().join(format!("lwp-find-tectonic-{}", std::process::id()));
        let nested = dir.join("drop");
        std::fs::create_dir_all(&nested).unwrap();
        let bin = nested.join(if cfg!(windows) {
            "tectonic.exe"
        } else {
            "tectonic"
        });
        std::fs::write(&bin, b"not a real binary").unwrap();
        let found = find_downloaded(&dir, 2).unwrap();
        assert_eq!(found, bin);
        let _ = std::fs::remove_dir_all(dir);
    }
}
