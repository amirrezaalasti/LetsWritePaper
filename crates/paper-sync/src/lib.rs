//! Cross-device sync for LetsWritePaper.
//!
//! A paper is shared through a relay (`paper serve`, or hosted by the desktop
//! app on the local network). Each device keeps its full local history and
//! exchanges change records with the relay; documents merge as CRDTs, so
//! devices can edit concurrently and offline.

pub mod client;
pub mod server;

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
pub use client::{join, lan_ip, parse_link, share, start, HostedRelay, RemoteConfig, SyncHandle, SyncStatus};
pub use server::{Peer, Relay};

pub const DEFAULT_PORT: u16 = 8787;

/// Host a relay on this machine (all interfaces) and share `project` through
/// it, so devices on the same network can join. Returns the relay and the
/// saved configuration (use `public_link()` for the link to hand out).
pub fn share_on_this_network(project: &paper_core::Project, data: &Path, port: u16) -> Result<(Arc<Relay>, RemoteConfig)> {
    let secret = uuid::Uuid::new_v4().simple().to_string();
    let relay = Relay::new(data, Some(secret.clone()))?;
    let port = relay.spawn(&format!("0.0.0.0:{port}"))?;
    let mut cfg = share(project, &format!("http://127.0.0.1:{port}"), Some(secret.as_str()))?;
    // Others reach us by LAN address; we keep talking to ourselves locally.
    cfg.hosted = Some(HostedRelay { port, data: data.to_path_buf(), secret });
    cfg.save(&project.root)?;
    Ok((relay, cfg))
}

/// Restart the relay for a project this machine hosts (after an app restart).
pub fn resume_hosting(cfg: &RemoteConfig) -> Result<Option<(Arc<Relay>, String)>> {
    let Some(h) = &cfg.hosted else { return Ok(None) };
    let relay = Relay::new(&h.data, Some(h.secret.clone()))?;
    relay.spawn(&format!("0.0.0.0:{}", h.port))?;
    Ok(Some((relay, cfg.public_link())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use paper_core::{Author, Format, Ownership, Project, Workspace};
    use std::time::{Duration, Instant};

    fn wait_for(mut cond: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(15) {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    #[test]
    fn share_join_and_live_sync_through_relay() {
        std::env::set_var("PAPER_NO_CLAUDE_SKILLS", "1");
        let relay_dir = tempfile::tempdir().unwrap();
        let relay = Relay::new(relay_dir.path(), Some("s3cret".into())).unwrap();
        let port = relay.spawn("127.0.0.1:0").unwrap();
        let server = format!("http://127.0.0.1:{port}");

        // Device A creates and shares a paper.
        let a_dir = tempfile::tempdir().unwrap();
        let a_proj = Project::init(a_dir.path(), "Relay Paper", Format::Typst).unwrap();
        let mut a = Workspace::open(a_proj.clone(), Ownership::Auto).unwrap();
        a.edit("main.typ", "= Method <sec:method>\n", "= Method <sec:method>\nFrom laptop.\n", None, &Author::human("ada"), None).unwrap();
        assert!(share(&a_proj, &server, None).is_err(), "secret required to create");
        let cfg = share(&a_proj, &server, Some("s3cret")).unwrap();
        let link = cfg.link();
        assert_eq!(parse_link(&link).unwrap().1, cfg.project);

        // Device B joins from the link and gets everything.
        let b_dir = tempfile::tempdir().unwrap();
        let b_root = b_dir.path().join("paper");
        let b_proj = join(&link, &b_root).unwrap();
        let mut b = Workspace::open(b_proj, Ownership::Auto).unwrap();
        assert!(b.text("main.typ").unwrap().contains("From laptop."));
        assert_eq!(b.project.manifest.paper.title, "Relay Paper");

        // Both run background sync; edits flow both ways.
        let ha = start(&a.project.root).unwrap().unwrap();
        let hb = start(&b.project.root).unwrap().unwrap();
        b.edit("main.typ", "From laptop.\n", "From laptop.\nFrom tablet.\n", None, &Author::human("ada"), None).unwrap();
        assert!(wait_for(|| {
            a.poll().unwrap();
            a.text("main.typ").unwrap().contains("From tablet.")
        }));
        a.bind_skill(Some("method*"), "paper-methods").unwrap();
        let round = a.start_review("Shared review", None, None, &Author::agent("claude-code")).unwrap();
        a.add_review_comment(round.id, "main.typ", "From tablet.", "minor", "clarity", "Which tablet?", None).unwrap();
        assert!(wait_for(|| {
            b.poll().unwrap();
            b.review_comments(&Default::default()).unwrap().len() == 1
        }));
        assert!(wait_for(|| std::fs::read_to_string(b.project.root.join("paper.toml")).unwrap().contains("paper-methods")));
        assert!(ha.snapshot().connected && hb.snapshot().connected);
        // Each device sees the other one, not itself.
        assert!(wait_for(|| ha.snapshot().peers.len() == 1 && hb.snapshot().peers.len() == 1));
        // Disk mirrors on both sides.
        assert!(std::fs::read_to_string(a.project.root.join("main.typ")).unwrap().contains("From tablet."));

        // Joining into a folder inside a paper project would sync it into itself.
        assert!(join(&link, &a.project.root.join("copy")).is_err());

        // A bad token is rejected.
        let bad = format!("{server}/p/{}#nope", cfg.project);
        assert!(join(&bad, &b_dir.path().join("other")).is_err());
    }
}
