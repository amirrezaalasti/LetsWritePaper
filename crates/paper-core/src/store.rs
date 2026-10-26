//! Durable, multi-process store (`.paper/history.db`, SQLite in WAL mode).
//!
//! The `updates` table is an append-only log of CRDT updates. Several processes
//! (the desktop app, MCP servers, the CLI) append to it concurrently; each one
//! tails rows written by the others to stay in sync.
//!
//! Every shareable record also has a globally unique `uid`, and every local
//! change is noted in `changelog`, so `sync` can exchange records with other
//! devices through a relay without caring about local row ids.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::author::{Author, AuthorKind};
use crate::review::{ReviewComment, ReviewRound, Scores};
use crate::{now_ms, Error, Result};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS documents (
    path TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS updates (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    uid TEXT,
    doc TEXT NOT NULL,
    author_id TEXT NOT NULL,
    author_kind TEXT NOT NULL,
    author_name TEXT NOT NULL,
    session TEXT NOT NULL,
    origin TEXT NOT NULL,
    message TEXT,
    summary TEXT,
    added INTEGER NOT NULL DEFAULT 0,
    removed INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    data BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS updates_doc ON updates(doc, id);
CREATE TABLE IF NOT EXISTS checkpoints (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    uid TEXT,
    name TEXT NOT NULL UNIQUE,
    message TEXT NOT NULL,
    kind TEXT NOT NULL,
    update_id INTEGER NOT NULL,
    author_id TEXT NOT NULL,
    author_kind TEXT NOT NULL,
    author_name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    git_commit TEXT
);
CREATE TABLE IF NOT EXISTS checkpoint_docs (
    checkpoint_uid TEXT NOT NULL,
    doc TEXT NOT NULL,
    state BLOB NOT NULL,
    PRIMARY KEY (checkpoint_uid, doc)
);
CREATE TABLE IF NOT EXISTS proposals (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    uid TEXT,
    doc TEXT NOT NULL,
    old_text TEXT NOT NULL,
    new_text TEXT NOT NULL,
    rationale TEXT NOT NULL,
    author_id TEXT NOT NULL,
    author_kind TEXT NOT NULL,
    author_name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    created_at INTEGER NOT NULL,
    resolved_at INTEGER,
    resolved_by TEXT,
    updated_at INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS review_rounds (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    uid TEXT,
    title TEXT NOT NULL,
    persona TEXT,
    venue TEXT,
    author_id TEXT NOT NULL,
    author_kind TEXT NOT NULL,
    author_name TEXT NOT NULL,
    update_id INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'open',
    summary TEXT,
    scores TEXT,
    created_at INTEGER NOT NULL,
    finished_at INTEGER,
    updated_at INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS review_comments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    uid TEXT,
    round_id INTEGER NOT NULL REFERENCES review_rounds(id),
    round_uid TEXT,
    doc TEXT NOT NULL,
    section TEXT,
    quote TEXT NOT NULL,
    offset_hint INTEGER,
    severity TEXT NOT NULL,
    category TEXT NOT NULL,
    body TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'open',
    response TEXT,
    responder_id TEXT,
    responder_kind TEXT,
    responder_name TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS project_files (
    path TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS changelog (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    key TEXT NOT NULL,
    origin TEXT
);
CREATE INDEX IF NOT EXISTS changelog_key ON changelog(kind, key, seq);
"#;

/// Columns added after the first release, for databases created before them.
const ADDED_COLUMNS: &[(&str, &str, &str)] = &[
    ("documents", "updated_at", "INTEGER NOT NULL DEFAULT 0"),
    ("updates", "uid", "TEXT"),
    ("updates", "summary", "TEXT"),
    ("checkpoints", "uid", "TEXT"),
    ("proposals", "uid", "TEXT"),
    ("proposals", "updated_at", "INTEGER NOT NULL DEFAULT 0"),
    ("review_rounds", "uid", "TEXT"),
    ("review_rounds", "updated_at", "INTEGER NOT NULL DEFAULT 0"),
    ("review_comments", "uid", "TEXT"),
    ("review_comments", "round_uid", "TEXT"),
];

const UID_TABLES: &[&str] = &[
    "updates",
    "checkpoints",
    "proposals",
    "review_rounds",
    "review_comments",
];

pub fn new_uid() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Metadata of one stored CRDT update (without the payload).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateMeta {
    pub id: i64,
    pub doc: String,
    pub author: Author,
    pub session: String,
    pub origin: String,
    pub message: Option<String>,
    /// One-line description of the text that changed. Filled for every edit.
    pub summary: Option<String>,
    pub added: i64,
    pub removed: i64,
    pub created_at: i64,
}

#[derive(Clone, Debug)]
pub struct UpdateRecord {
    pub meta: UpdateMeta,
    pub data: Vec<u8>,
}

pub struct NewUpdate<'a> {
    pub doc: &'a str,
    pub author: &'a Author,
    pub session: &'a str,
    pub origin: &'a str,
    pub message: Option<&'a str>,
    pub summary: Option<&'a str>,
    pub added: i64,
    pub removed: i64,
    pub data: &'a [u8],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: i64,
    pub uid: String,
    pub name: String,
    pub message: String,
    /// `checkpoint` or a milestone such as `submitted`, `camera-ready`, `rebuttal`, `arxiv`.
    pub kind: String,
    /// Local history position when the checkpoint was taken (on the device that took it).
    pub update_id: i64,
    pub author: Author,
    pub created_at: i64,
    pub git_commit: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proposal {
    pub id: i64,
    pub uid: String,
    pub doc: String,
    pub old_text: String,
    pub new_text: String,
    pub rationale: String,
    pub author: Author,
    /// `pending`, `accepted`, `rejected` or `stale` (the old text no longer exists).
    pub status: String,
    pub created_at: i64,
    pub resolved_at: Option<i64>,
    pub resolved_by: Option<String>,
}

pub struct Store {
    pub(crate) conn: Connection,
}

pub(crate) fn author_from(row: &Row, idx: usize) -> rusqlite::Result<Author> {
    Ok(Author {
        id: row.get(idx)?,
        kind: AuthorKind::parse(&row.get::<_, String>(idx + 1)?),
        name: row.get(idx + 2)?,
    })
}

const UPDATE_META_COLS: &str =
    "id, doc, author_id, author_kind, author_name, session, origin, message, added, removed, created_at, summary";

fn update_meta(row: &Row) -> rusqlite::Result<UpdateMeta> {
    Ok(UpdateMeta {
        id: row.get(0)?,
        doc: row.get(1)?,
        author: author_from(row, 2)?,
        session: row.get(5)?,
        origin: row.get(6)?,
        message: row.get(7)?,
        added: row.get(8)?,
        removed: row.get(9)?,
        created_at: row.get(10)?,
        summary: row.get(11)?,
    })
}

const CHECKPOINT_COLS: &str =
    "id, name, message, kind, update_id, author_id, author_kind, author_name, created_at, git_commit, uid";

fn checkpoint(row: &Row) -> rusqlite::Result<Checkpoint> {
    Ok(Checkpoint {
        id: row.get(0)?,
        name: row.get(1)?,
        message: row.get(2)?,
        kind: row.get(3)?,
        update_id: row.get(4)?,
        author: author_from(row, 5)?,
        created_at: row.get(8)?,
        git_commit: row.get(9)?,
        uid: row.get(10)?,
    })
}

const PROPOSAL_COLS: &str = "id, doc, old_text, new_text, rationale, author_id, author_kind, author_name, status, created_at, resolved_at, resolved_by, uid";

fn proposal(row: &Row) -> rusqlite::Result<Proposal> {
    Ok(Proposal {
        id: row.get(0)?,
        doc: row.get(1)?,
        old_text: row.get(2)?,
        new_text: row.get(3)?,
        rationale: row.get(4)?,
        author: author_from(row, 5)?,
        status: row.get(8)?,
        created_at: row.get(9)?,
        resolved_at: row.get(10)?,
        resolved_by: row.get(11)?,
        uid: row.get(12)?,
    })
}

const ROUND_COLS: &str = "id, title, persona, venue, author_id, author_kind, author_name, update_id, status, summary, scores, created_at, finished_at, uid";

fn round(row: &Row) -> rusqlite::Result<ReviewRound> {
    let scores: Option<String> = row.get(10)?;
    Ok(ReviewRound {
        id: row.get(0)?,
        title: row.get(1)?,
        persona: row.get(2)?,
        venue: row.get(3)?,
        reviewer: author_from(row, 4)?,
        update_id: row.get(7)?,
        status: row.get(8)?,
        summary: row.get(9)?,
        scores: scores.and_then(|s| serde_json::from_str::<Scores>(&s).ok()),
        created_at: row.get(11)?,
        finished_at: row.get(12)?,
        uid: row.get(13)?,
    })
}

const COMMENT_COLS: &str = "id, round_id, doc, section, quote, offset_hint, severity, category, body, status, response, responder_id, responder_kind, responder_name, created_at, updated_at, uid, round_uid";

fn comment(row: &Row) -> rusqlite::Result<ReviewComment> {
    let responder_id: Option<String> = row.get(11)?;
    let responder = match responder_id {
        Some(_) => Some(author_from(row, 11)?),
        None => None,
    };
    Ok(ReviewComment {
        id: row.get(0)?,
        round_id: row.get(1)?,
        doc: row.get(2)?,
        section: row.get(3)?,
        quote: row.get(4)?,
        offset_hint: row.get(5)?,
        severity: row.get(6)?,
        category: row.get(7)?,
        body: row.get(8)?,
        status: row.get(9)?,
        response: row.get(10)?,
        responder,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
        uid: row.get(16)?,
        round_uid: row.get(17)?,
        current_line: None,
    })
}

/// Filter for listing review comments.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CommentFilter {
    pub round_id: Option<i64>,
    pub status: Option<String>,
    pub doc: Option<String>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let store = Store {
            conn: Connection::open_in_memory()?,
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(SCHEMA)?;
        for (table, column, decl) in ADDED_COLUMNS {
            let exists: bool = self.conn.query_row(
                &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1"),
                params![column],
                |r| r.get::<_, i64>(0).map(|n| n > 0),
            )?;
            if !exists {
                self.conn
                    .execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"))?;
            }
        }
        for table in UID_TABLES {
            self.conn.execute(
                &format!("UPDATE {table} SET uid = lower(hex(randomblob(16))) WHERE uid IS NULL"),
                [],
            )?;
            self.conn.execute_batch(&format!(
                "CREATE UNIQUE INDEX IF NOT EXISTS {table}_uid ON {table}(uid)"
            ))?;
        }
        self.conn.execute(
            "UPDATE review_comments SET round_uid = (SELECT uid FROM review_rounds r WHERE r.id = round_id) WHERE round_uid IS NULL",
            [],
        )?;
        Ok(())
    }

    /// Changes whenever another connection commits; cheap to poll.
    pub fn data_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("PRAGMA data_version", [], |r| r.get(0))?)
    }

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Note a local change for sync. `origin` identifies a remote device when the
    /// change arrived through a relay (so it is not echoed back to it).
    pub(crate) fn log_change(&self, kind: &str, key: &str, origin: Option<&str>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO changelog(kind, key, origin) VALUES (?1, ?2, ?3)",
            params![kind, key, origin],
        )?;
        Ok(())
    }

    // ---- documents -------------------------------------------------------

    pub fn documents(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM documents WHERE deleted = 0 ORDER BY path")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Returns true when the document row was newly created (or revived).
    pub fn add_document(&self, path: &str) -> Result<bool> {
        let now = now_ms();
        let n = self.conn.execute(
            "INSERT INTO documents(path, created_at, updated_at) VALUES (?1, ?2, ?2)
             ON CONFLICT(path) DO UPDATE SET deleted = 0, updated_at = ?2 WHERE deleted = 1",
            params![path, now],
        )?;
        if n > 0 {
            self.log_change("document", path, None)?;
        }
        Ok(n > 0)
    }

    pub fn remove_document(&self, path: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE documents SET deleted = 1, updated_at = ?2 WHERE path = ?1",
            params![path, now_ms()],
        )?;
        self.log_change("document", path, None)
    }

    // ---- updates ---------------------------------------------------------

    pub fn insert_update(&self, u: &NewUpdate) -> Result<i64> {
        let uid = new_uid();
        self.conn.execute(
            "INSERT INTO updates(uid, doc, author_id, author_kind, author_name, session, origin, message, summary, added, removed, created_at, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                uid,
                u.doc,
                u.author.id,
                u.author.kind.as_str(),
                u.author.name,
                u.session,
                u.origin,
                u.message,
                u.summary,
                u.added,
                u.removed,
                now_ms(),
                u.data
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.log_change("update", &uid, None)?;
        Ok(id)
    }

    /// All update payloads for a document, optionally only up to `upto` (inclusive).
    pub fn doc_updates(&self, doc: &str, upto: Option<i64>) -> Result<Vec<Vec<u8>>> {
        let mut stmt = self
            .conn
            .prepare("SELECT data FROM updates WHERE doc = ?1 AND id <= ?2 ORDER BY id")?;
        let rows = stmt.query_map(params![doc, upto.unwrap_or(i64::MAX)], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Updates written after `after` by other processes (or other devices).
    pub fn updates_since(&self, after: i64, exclude_origin: &str) -> Result<Vec<UpdateRecord>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {UPDATE_META_COLS}, data FROM updates WHERE id > ?1 AND origin != ?2 ORDER BY id"
        ))?;
        let rows = stmt.query_map(params![after, exclude_origin], |r| {
            Ok(UpdateRecord {
                meta: update_meta(r)?,
                data: r.get(12)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn max_update_id(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COALESCE(MAX(id), 0) FROM updates", [], |r| r.get(0))?)
    }

    /// Newest-first update metadata, optionally for one document and one author.
    pub fn recent(
        &self,
        doc: Option<&str>,
        author: Option<&str>,
        limit: usize,
    ) -> Result<Vec<UpdateMeta>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {UPDATE_META_COLS} FROM updates
             WHERE (?1 IS NULL OR doc = ?1) AND (?2 IS NULL OR author_id = ?2)
             ORDER BY id DESC LIMIT ?3"
        ))?;
        let rows = stmt.query_map(params![doc, author, limit as i64], update_meta)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Update payloads in history order, used to describe edits that predate summaries.
    pub fn doc_payloads(&self, doc: &str) -> Result<Vec<(i64, Vec<u8>)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, data FROM updates WHERE doc = ?1 ORDER BY id")?;
        let rows = stmt.query_map(params![doc], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Remember a derived summary. Not part of the sync log: other devices can rebuild it.
    pub fn set_summary(&self, id: i64, summary: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE updates SET summary = ?2 WHERE id = ?1 AND summary IS NULL",
            params![id, summary],
        )?;
        Ok(())
    }

    /// Newest-first update metadata, optionally for one document and before an id.
    pub fn log(
        &self,
        doc: Option<&str>,
        before: Option<i64>,
        limit: usize,
    ) -> Result<Vec<UpdateMeta>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {UPDATE_META_COLS} FROM updates
             WHERE (?1 IS NULL OR doc = ?1) AND id < ?2 ORDER BY id DESC LIMIT ?3"
        ))?;
        let rows = stmt.query_map(
            params![doc, before.unwrap_or(i64::MAX), limit as i64],
            update_meta,
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn update_meta(&self, id: i64) -> Result<Option<UpdateMeta>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {UPDATE_META_COLS} FROM updates WHERE id = ?1"),
                params![id],
                update_meta,
            )
            .optional()?)
    }

    /// Distinct authors who changed anything in (from, to].
    pub fn authors_between(&self, from: i64, to: i64) -> Result<Vec<Author>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT author_id, author_kind, author_name FROM updates WHERE id > ?1 AND id <= ?2",
        )?;
        let rows = stmt.query_map(params![from, to], |r| author_from(r, 0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every document that ever had content, including deleted ones.
    pub fn all_doc_paths(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT doc FROM updates ORDER BY doc")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Everyone who has ever written to the project, keyed by author id.
    pub fn authors(&self) -> Result<std::collections::HashMap<String, Author>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT author_id, author_kind, author_name FROM updates")?;
        let rows = stmt.query_map([], |r| author_from(r, 0))?;
        let mut map = std::collections::HashMap::new();
        for a in rows {
            let a = a?;
            map.insert(a.id.clone(), a);
        }
        Ok(map)
    }

    // ---- checkpoints -----------------------------------------------------

    /// Record a checkpoint together with a full CRDT snapshot of every
    /// document, so the version can be reconstructed on any device.
    pub fn insert_checkpoint(
        &self,
        name: &str,
        message: &str,
        kind: &str,
        update_id: i64,
        author: &Author,
        snapshots: &[(String, Vec<u8>)],
    ) -> Result<Checkpoint> {
        let uid = new_uid();
        self.conn
            .execute(
                "INSERT INTO checkpoints(uid, name, message, kind, update_id, author_id, author_kind, author_name, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![uid, name, message, kind, update_id, author.id, author.kind.as_str(), author.name, now_ms()],
            )
            .map_err(|e| match e {
                rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => {
                    Error::Invalid(format!("a checkpoint named {name:?} already exists"))
                }
                e => e.into(),
            })?;
        let id = self.conn.last_insert_rowid();
        for (doc, state) in snapshots {
            self.conn.execute(
                "INSERT OR REPLACE INTO checkpoint_docs(checkpoint_uid, doc, state) VALUES (?1, ?2, ?3)",
                params![uid, doc, state],
            )?;
        }
        self.log_change("checkpoint", &uid, None)?;
        self.checkpoint(&id.to_string())?
            .ok_or_else(|| Error::NotFound(name.into()))
    }

    /// The CRDT state of `doc` at a checkpoint (None: not in that checkpoint,
    /// or an old checkpoint taken before snapshots existed).
    pub fn checkpoint_snapshot(&self, checkpoint_uid: &str, doc: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row(
                "SELECT state FROM checkpoint_docs WHERE checkpoint_uid = ?1 AND doc = ?2",
                params![checkpoint_uid, doc],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn checkpoint_has_snapshots(&self, checkpoint_uid: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM checkpoint_docs WHERE checkpoint_uid = ?1",
            params![checkpoint_uid],
            |r| r.get::<_, i64>(0).map(|n| n > 0),
        )?)
    }

    pub fn set_checkpoint_commit(&self, id: i64, sha: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE checkpoints SET git_commit = ?2 WHERE id = ?1",
            params![id, sha],
        )?;
        Ok(())
    }

    /// Look a checkpoint up by name, or by numeric id.
    pub fn checkpoint(&self, name_or_id: &str) -> Result<Option<Checkpoint>> {
        let by_name = self
            .conn
            .query_row(
                &format!("SELECT {CHECKPOINT_COLS} FROM checkpoints WHERE name = ?1"),
                params![name_or_id],
                checkpoint,
            )
            .optional()?;
        if by_name.is_some() {
            return Ok(by_name);
        }
        match name_or_id.parse::<i64>() {
            Ok(id) => Ok(self
                .conn
                .query_row(
                    &format!("SELECT {CHECKPOINT_COLS} FROM checkpoints WHERE id = ?1"),
                    params![id],
                    checkpoint,
                )
                .optional()?),
            Err(_) => Ok(None),
        }
    }

    pub fn checkpoints(&self) -> Result<Vec<Checkpoint>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {CHECKPOINT_COLS} FROM checkpoints ORDER BY created_at DESC, id DESC"
        ))?;
        let rows = stmt.query_map([], checkpoint)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---- proposals -------------------------------------------------------

    pub fn insert_proposal(
        &self,
        doc: &str,
        old: &str,
        new: &str,
        rationale: &str,
        author: &Author,
    ) -> Result<Proposal> {
        let uid = new_uid();
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO proposals(uid, doc, old_text, new_text, rationale, author_id, author_kind, author_name, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
            params![uid, doc, old, new, rationale, author.id, author.kind.as_str(), author.name, now],
        )?;
        let id = self.conn.last_insert_rowid();
        self.log_change("proposal", &uid, None)?;
        self.proposal(id)
    }

    pub fn proposal(&self, id: i64) -> Result<Proposal> {
        self.conn
            .query_row(
                &format!("SELECT {PROPOSAL_COLS} FROM proposals WHERE id = ?1"),
                params![id],
                proposal,
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("proposal #{id}")))
    }

    pub fn proposals(&self, status: Option<&str>) -> Result<Vec<Proposal>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {PROPOSAL_COLS} FROM proposals WHERE (?1 IS NULL OR status = ?1) ORDER BY created_at DESC, id DESC"
        ))?;
        let rows = stmt.query_map(params![status], proposal)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn resolve_proposal(&self, id: i64, status: &str, by: &Author) -> Result<()> {
        let now = now_ms();
        self.conn.execute(
            "UPDATE proposals SET status = ?2, resolved_at = ?3, resolved_by = ?4, updated_at = ?3 WHERE id = ?1",
            params![id, status, now, by.id],
        )?;
        let uid = self.proposal(id)?.uid;
        self.log_change("proposal", &uid, None)
    }

    // ---- reviews ---------------------------------------------------------

    pub fn insert_round(
        &self,
        title: &str,
        persona: Option<&str>,
        venue: Option<&str>,
        reviewer: &Author,
        update_id: i64,
    ) -> Result<ReviewRound> {
        let uid = new_uid();
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO review_rounds(uid, title, persona, venue, author_id, author_kind, author_name, update_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
            params![uid, title, persona, venue, reviewer.id, reviewer.kind.as_str(), reviewer.name, update_id, now],
        )?;
        let id = self.conn.last_insert_rowid();
        self.log_change("round", &uid, None)?;
        self.round(id)
    }

    pub fn round(&self, id: i64) -> Result<ReviewRound> {
        self.conn
            .query_row(
                &format!("SELECT {ROUND_COLS} FROM review_rounds WHERE id = ?1"),
                params![id],
                round,
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("review round #{id}")))
    }

    pub fn rounds(&self) -> Result<Vec<ReviewRound>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ROUND_COLS} FROM review_rounds ORDER BY created_at DESC, id DESC"
        ))?;
        let rows = stmt.query_map([], round)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn finish_round(
        &self,
        id: i64,
        summary: &str,
        scores: Option<&Scores>,
    ) -> Result<ReviewRound> {
        let scores = scores.map(serde_json::to_string).transpose()?;
        let now = now_ms();
        self.conn.execute(
            "UPDATE review_rounds SET status = 'finished', summary = ?2, scores = ?3, finished_at = ?4, updated_at = ?4 WHERE id = ?1",
            params![id, summary, scores, now],
        )?;
        let r = self.round(id)?;
        self.log_change("round", &r.uid, None)?;
        Ok(r)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_comment(
        &self,
        round_id: i64,
        doc: &str,
        section: Option<&str>,
        quote: &str,
        offset_hint: Option<i64>,
        severity: &str,
        category: &str,
        body: &str,
    ) -> Result<ReviewComment> {
        let now = now_ms();
        let uid = new_uid();
        let round_uid = self.round(round_id)?.uid;
        self.conn.execute(
            "INSERT INTO review_comments(uid, round_id, round_uid, doc, section, quote, offset_hint, severity, category, body, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
            params![uid, round_id, round_uid, doc, section, quote, offset_hint, severity, category, body, now],
        )?;
        let id = self.conn.last_insert_rowid();
        self.log_change("comment", &uid, None)?;
        self.comment(id)
    }

    pub fn comment(&self, id: i64) -> Result<ReviewComment> {
        self.conn
            .query_row(
                &format!("SELECT {COMMENT_COLS} FROM review_comments WHERE id = ?1"),
                params![id],
                comment,
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("review comment #{id}")))
    }

    pub fn comments(&self, f: &CommentFilter) -> Result<Vec<ReviewComment>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COMMENT_COLS} FROM review_comments
             WHERE (?1 IS NULL OR round_id = ?1) AND (?2 IS NULL OR status = ?2) AND (?3 IS NULL OR doc = ?3)
             ORDER BY round_id, created_at, id"
        ))?;
        let rows = stmt.query_map(params![f.round_id, f.status, f.doc], comment)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn respond_comment(
        &self,
        id: i64,
        response: &str,
        status: &str,
        by: &Author,
    ) -> Result<ReviewComment> {
        self.conn.execute(
            "UPDATE review_comments SET response = ?2, status = ?3, responder_id = ?4, responder_kind = ?5,
             responder_name = ?6, updated_at = ?7 WHERE id = ?1",
            params![id, response, status, by.id, by.kind.as_str(), by.name, now_ms()],
        )?;
        let c = self.comment(id)?;
        self.log_change("comment", &c.uid, None)?;
        Ok(c)
    }

    // ---- shared project files (paper.toml, project skills) ---------------------

    pub fn file(&self, path: &str) -> Result<Option<(String, bool, i64)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT content, deleted, updated_at FROM project_files WHERE path = ?1",
                params![path],
                |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?)),
            )
            .optional()?)
    }

    pub fn files(&self) -> Result<Vec<(String, String, bool)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, content, deleted FROM project_files ORDER BY path")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Record the current content of a shared project file (a local change).
    pub fn put_file(&self, path: &str, content: &str, deleted: bool) -> Result<bool> {
        if let Some((old, old_deleted, _)) = self.file(path)? {
            if old == content && old_deleted == deleted {
                return Ok(false);
            }
        }
        self.conn.execute(
            "INSERT INTO project_files(path, content, deleted, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET content = excluded.content, deleted = excluded.deleted, updated_at = excluded.updated_at",
            params![path, content, deleted as i64, now_ms()],
        )?;
        self.log_change("file", path, None)?;
        Ok(true)
    }
}
