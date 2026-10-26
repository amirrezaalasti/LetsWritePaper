//! Core engine for LetsWritePaper.
//!
//! Every change to a paper — typed by a human in the app, made by an agent over
//! MCP, or written to disk by an external tool — becomes a Yjs-compatible CRDT
//! update stored in an append-only, author-attributed log (`.paper/history.db`).
//! That log is simultaneously the sync channel between processes, the undo-able
//! history, and the source for blame, diffs, checkpoints and reviews.

pub mod agents;
pub mod author;
pub mod doc;
pub mod error;
pub mod git;
pub mod history;
pub mod outline;
pub mod project;
pub mod review;
pub mod skills;
pub mod store;
pub mod sync;
pub mod textdiff;
pub mod workspace;

pub use author::{Author, AuthorKind};
pub use error::{Error, Result};
pub use project::{Format, Manifest, Project};
pub use workspace::{Ownership, Resolved, Workspace, WorkspaceEvent};

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// RFC 3339 rendering of a millisecond timestamp, in UTC.
pub fn format_ms(ms: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp_nanos(ms as i128 * 1_000_000)
        .ok()
        .and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_else(|| ms.to_string())
}
