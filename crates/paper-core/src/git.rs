//! Optional git interop: checkpoints become git commits (when the project is a
//! repository), with every contributing human and agent credited as a co-author.

use std::path::Path;
use std::process::Command;

use crate::author::{Author, AuthorKind};

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn is_repo(root: &Path) -> bool {
    git(root, &["rev-parse", "--is-inside-work-tree"]).as_deref() == Some("true")
}

/// Stage everything and commit. Returns the new commit sha, or None when git is
/// unavailable, this is not a repository, or there was nothing to commit.
pub fn commit_all(
    root: &Path,
    subject: &str,
    body: &str,
    contributors: &[Author],
) -> Option<String> {
    if !is_repo(root) {
        return None;
    }
    git(root, &["add", "-A"])?;
    let mut message = format!("{subject}\n");
    if !body.trim().is_empty() {
        message.push_str(&format!("\n{}\n", body.trim()));
    }
    let trailers: Vec<String> = contributors
        .iter()
        .filter(|a| matches!(a.kind, AuthorKind::Human | AuthorKind::Agent))
        .map(|a| {
            format!(
                "Co-Authored-By: {} <{}@letswritepaper.local>",
                a.name,
                a.id.replace(':', ".")
            )
        })
        .collect();
    if !trailers.is_empty() {
        message.push_str(&format!("\n{}\n", trailers.join("\n")));
    }
    git(root, &["commit", "-q", "-m", &message])?;
    git(root, &["rev-parse", "HEAD"])
}
