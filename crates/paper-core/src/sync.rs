//! Device-to-device sync records.
//!
//! Every local change is noted in the store's `changelog`. `export_changes`
//! turns changelog entries into self-contained [`Record`]s keyed by global ids;
//! `import_records` applies records from another device. CRDT updates are
//! immutable and merge by construction; mutable records (proposal status,
//! review responses, shared files) are last-writer-wins on `updated_at`.
//!
//! Imported updates land in the local log like rows written by another
//! process, so every open `Workspace` picks them up through `poll`.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::project::Project;
use crate::store::Store;
use crate::{Error, Result};

/// `origin` written on updates that arrived through sync.
pub const SYNC_ORIGIN: &str = "sync";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AuthorCols {
    pub id: String,
    pub kind: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    Document {
        path: String,
        created_at: i64,
        deleted: bool,
        updated_at: i64,
    },
    Update {
        uid: String,
        doc: String,
        author: AuthorCols,
        session: String,
        message: Option<String>,
        added: i64,
        removed: i64,
        created_at: i64,
        /// Base64 Yjs v1 update.
        data: String,
    },
    Checkpoint {
        uid: String,
        name: String,
        message: String,
        checkpoint_kind: String,
        author: AuthorCols,
        created_at: i64,
        /// (document, base64 CRDT state) at the checkpoint.
        docs: Vec<(String, String)>,
    },
    Proposal {
        uid: String,
        doc: String,
        old_text: String,
        new_text: String,
        rationale: String,
        author: AuthorCols,
        status: String,
        created_at: i64,
        resolved_at: Option<i64>,
        resolved_by: Option<String>,
        updated_at: i64,
    },
    Round {
        uid: String,
        title: String,
        persona: Option<String>,
        venue: Option<String>,
        author: AuthorCols,
        status: String,
        summary: Option<String>,
        scores: Option<String>,
        created_at: i64,
        finished_at: Option<i64>,
        updated_at: i64,
    },
    Comment {
        uid: String,
        round_uid: String,
        doc: String,
        section: Option<String>,
        quote: String,
        offset_hint: Option<i64>,
        severity: String,
        category: String,
        body: String,
        status: String,
        response: Option<String>,
        responder: Option<AuthorCols>,
        created_at: i64,
        updated_at: i64,
    },
    File {
        path: String,
        content: String,
        deleted: bool,
        updated_at: i64,
    },
}

impl Record {
    /// Import order: parents before children.
    fn rank(&self) -> u8 {
        match self {
            Record::Document { .. } => 0,
            Record::Update { .. } => 1,
            Record::Checkpoint { .. } => 2,
            Record::Proposal { .. } => 3,
            Record::Round { .. } => 4,
            Record::Comment { .. } => 5,
            Record::File { .. } => 6,
        }
    }

    fn change_key(&self) -> (&'static str, String) {
        match self {
            Record::Document { path, .. } => ("document", path.clone()),
            Record::Update { uid, .. } => ("update", uid.clone()),
            Record::Checkpoint { uid, .. } => ("checkpoint", uid.clone()),
            Record::Proposal { uid, .. } => ("proposal", uid.clone()),
            Record::Round { uid, .. } => ("round", uid.clone()),
            Record::Comment { uid, .. } => ("comment", uid.clone()),
            Record::File { path, .. } => ("file", path.clone()),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChangeBatch {
    pub records: Vec<Record>,
    /// Cursor to pass as `after` next time.
    pub seq: i64,
    /// True when more changes are waiting beyond this batch.
    pub more: bool,
}

/// What an import changed locally.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ImportOutcome {
    pub changed: usize,
    /// Shared project files whose content changed (to write to disk).
    pub files: Vec<String>,
}

fn author_cols(row: &rusqlite::Row, idx: usize) -> rusqlite::Result<AuthorCols> {
    Ok(AuthorCols { id: row.get(idx)?, kind: row.get(idx + 1)?, name: row.get(idx + 2)? })
}

impl Store {
    pub fn max_seq(&self) -> Result<i64> {
        Ok(self.conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM changelog", [], |r| r.get(0))?)
    }

    /// Records changed after `after`, skipping changes that came from
    /// `exclude_origin` (so a device is not sent its own changes back).
    pub fn export_changes(&self, after: i64, exclude_origin: Option<&str>, limit: usize) -> Result<ChangeBatch> {
        let max = self.max_seq()?;
        let mut stmt = self.conn.prepare(
            // Each key's latest change decides: skip it if that came from `exclude_origin`.
            "SELECT kind, key, seq FROM changelog c
             WHERE seq > ?1 AND seq <= ?2
               AND seq = (SELECT MAX(seq) FROM changelog c2 WHERE c2.kind = c.kind AND c2.key = c.key AND c2.seq <= ?2)
               AND (?3 IS NULL OR origin IS NULL OR origin != ?3)
             ORDER BY seq LIMIT ?4",
        )?;
        let keys: Vec<(String, String, i64)> = stmt
            .query_map(params![after, max, exclude_origin, limit as i64 + 1], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let more = keys.len() > limit;
        let keys = &keys[..keys.len().min(limit)];
        let seq = if more { keys.last().map(|k| k.2).unwrap_or(max) } else { max };
        let mut records = vec![];
        for (kind, key, _) in keys {
            if let Some(r) = self.record(kind, key)? {
                records.push(r);
            }
        }
        Ok(ChangeBatch { records, seq, more })
    }

    /// Note every existing record in the changelog (used when a project is
    /// first shared, so data written before sync existed is included).
    pub fn backfill_changelog(&self) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch(
            "INSERT INTO changelog(kind, key) SELECT 'document', path FROM documents;
             INSERT INTO changelog(kind, key) SELECT 'update', uid FROM updates ORDER BY id;
             INSERT INTO changelog(kind, key) SELECT 'checkpoint', uid FROM checkpoints ORDER BY id;
             INSERT INTO changelog(kind, key) SELECT 'proposal', uid FROM proposals ORDER BY id;
             INSERT INTO changelog(kind, key) SELECT 'round', uid FROM review_rounds ORDER BY id;
             INSERT INTO changelog(kind, key) SELECT 'comment', uid FROM review_comments ORDER BY id;
             INSERT INTO changelog(kind, key) SELECT 'file', path FROM project_files;",
        )?;
        tx.commit()?;
        Ok(())
    }

    fn record(&self, kind: &str, key: &str) -> Result<Option<Record>> {
        let c = &self.conn;
        let r = match kind {
            "document" => c
                .query_row(
                    "SELECT path, created_at, deleted, updated_at FROM documents WHERE path = ?1",
                    params![key],
                    |r| Ok(Record::Document { path: r.get(0)?, created_at: r.get(1)?, deleted: r.get::<_, i64>(2)? != 0, updated_at: r.get(3)? }),
                )
                .optional()?,
            "update" => c
                .query_row(
                    "SELECT uid, doc, author_id, author_kind, author_name, session, message, added, removed, created_at, data FROM updates WHERE uid = ?1",
                    params![key],
                    |r| {
                        Ok(Record::Update {
                            uid: r.get(0)?,
                            doc: r.get(1)?,
                            author: author_cols(r, 2)?,
                            session: r.get(5)?,
                            message: r.get(6)?,
                            added: r.get(7)?,
                            removed: r.get(8)?,
                            created_at: r.get(9)?,
                            data: B64.encode(r.get::<_, Vec<u8>>(10)?),
                        })
                    },
                )
                .optional()?,
            "checkpoint" => {
                let base = c
                    .query_row(
                        "SELECT uid, name, message, kind, author_id, author_kind, author_name, created_at FROM checkpoints WHERE uid = ?1",
                        params![key],
                        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, author_cols(r, 4)?, r.get::<_, i64>(7)?)),
                    )
                    .optional()?;
                match base {
                    None => None,
                    Some((uid, name, message, checkpoint_kind, author, created_at)) => {
                        let mut stmt = c.prepare("SELECT doc, state FROM checkpoint_docs WHERE checkpoint_uid = ?1 ORDER BY doc")?;
                        let docs = stmt
                            .query_map(params![uid], |r| Ok((r.get::<_, String>(0)?, B64.encode(r.get::<_, Vec<u8>>(1)?))))?
                            .collect::<rusqlite::Result<_>>()?;
                        Some(Record::Checkpoint { uid, name, message, checkpoint_kind, author, created_at, docs })
                    }
                }
            }
            "proposal" => c
                .query_row(
                    "SELECT uid, doc, old_text, new_text, rationale, author_id, author_kind, author_name, status, created_at, resolved_at, resolved_by, updated_at FROM proposals WHERE uid = ?1",
                    params![key],
                    |r| {
                        Ok(Record::Proposal {
                            uid: r.get(0)?,
                            doc: r.get(1)?,
                            old_text: r.get(2)?,
                            new_text: r.get(3)?,
                            rationale: r.get(4)?,
                            author: author_cols(r, 5)?,
                            status: r.get(8)?,
                            created_at: r.get(9)?,
                            resolved_at: r.get(10)?,
                            resolved_by: r.get(11)?,
                            updated_at: r.get(12)?,
                        })
                    },
                )
                .optional()?,
            "round" => c
                .query_row(
                    "SELECT uid, title, persona, venue, author_id, author_kind, author_name, status, summary, scores, created_at, finished_at, updated_at FROM review_rounds WHERE uid = ?1",
                    params![key],
                    |r| {
                        Ok(Record::Round {
                            uid: r.get(0)?,
                            title: r.get(1)?,
                            persona: r.get(2)?,
                            venue: r.get(3)?,
                            author: author_cols(r, 4)?,
                            status: r.get(7)?,
                            summary: r.get(8)?,
                            scores: r.get(9)?,
                            created_at: r.get(10)?,
                            finished_at: r.get(11)?,
                            updated_at: r.get(12)?,
                        })
                    },
                )
                .optional()?,
            "comment" => c
                .query_row(
                    "SELECT uid, round_uid, doc, section, quote, offset_hint, severity, category, body, status, response, responder_id, responder_kind, responder_name, created_at, updated_at FROM review_comments WHERE uid = ?1",
                    params![key],
                    |r| {
                        let responder_id: Option<String> = r.get(11)?;
                        Ok(Record::Comment {
                            uid: r.get(0)?,
                            round_uid: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                            doc: r.get(2)?,
                            section: r.get(3)?,
                            quote: r.get(4)?,
                            offset_hint: r.get(5)?,
                            severity: r.get(6)?,
                            category: r.get(7)?,
                            body: r.get(8)?,
                            status: r.get(9)?,
                            response: r.get(10)?,
                            responder: match responder_id {
                                Some(_) => Some(author_cols(r, 11)?),
                                None => None,
                            },
                            created_at: r.get(14)?,
                            updated_at: r.get(15)?,
                        })
                    },
                )
                .optional()?,
            "file" => c
                .query_row(
                    "SELECT path, content, deleted, updated_at FROM project_files WHERE path = ?1",
                    params![key],
                    |r| Ok(Record::File { path: r.get(0)?, content: r.get(1)?, deleted: r.get::<_, i64>(2)? != 0, updated_at: r.get(3)? }),
                )
                .optional()?,
            _ => None,
        };
        Ok(r)
    }

    /// Apply records from another device. `origin` names that device so the
    /// resulting changelog entries are not sent back to it.
    pub fn import_records(&self, mut records: Vec<Record>, origin: &str) -> Result<ImportOutcome> {
        records.sort_by_key(Record::rank);
        let tx = self.conn.unchecked_transaction()?;
        let mut out = ImportOutcome::default();
        for rec in records {
            let (kind, key) = rec.change_key();
            let is_file = matches!(rec, Record::File { .. });
            if self.import_one(rec)? {
                out.changed += 1;
                self.log_change(kind, &key, Some(origin))?;
                if is_file {
                    out.files.push(key);
                }
            }
        }
        tx.commit()?;
        Ok(out)
    }

    fn local_updated_at(&self, table: &str, key_col: &str, key: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(&format!("SELECT updated_at FROM {table} WHERE {key_col} = ?1"), params![key], |r| r.get(0))
            .optional()?)
    }

    fn import_one(&self, rec: Record) -> Result<bool> {
        let c = &self.conn;
        let n = match rec {
            Record::Document { path, created_at, deleted, updated_at } => match self.local_updated_at("documents", "path", &path)? {
                Some(local) if local >= updated_at => 0,
                Some(_) => c.execute(
                    "UPDATE documents SET deleted = ?2, updated_at = ?3 WHERE path = ?1",
                    params![path, deleted as i64, updated_at],
                )?,
                None => c.execute(
                    "INSERT INTO documents(path, created_at, deleted, updated_at) VALUES (?1, ?2, ?3, ?4)",
                    params![path, created_at, deleted as i64, updated_at],
                )?,
            },
            Record::Update { uid, doc, author, session, message, added, removed, created_at, data } => {
                let data = B64.decode(data).map_err(|e| Error::Invalid(format!("bad update payload: {e}")))?;
                c.execute(
                    "INSERT INTO documents(path, created_at, updated_at) VALUES (?1, ?2, ?2) ON CONFLICT(path) DO NOTHING",
                    params![doc, created_at],
                )?;
                c.execute(
                    "INSERT OR IGNORE INTO updates(uid, doc, author_id, author_kind, author_name, session, origin, message, added, removed, created_at, data)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![uid, doc, author.id, author.kind, author.name, session, SYNC_ORIGIN, message, added, removed, created_at, data],
                )?
            }
            Record::Checkpoint { uid, name, message, checkpoint_kind, author, created_at, docs } => {
                let exists: bool = c.query_row("SELECT COUNT(*) FROM checkpoints WHERE uid = ?1", params![uid], |r| r.get::<_, i64>(0).map(|n| n > 0))?;
                if exists {
                    0
                } else {
                    // Two devices may pick the same name; keep both.
                    let mut final_name = name.clone();
                    let mut i = 2;
                    while c.query_row("SELECT COUNT(*) FROM checkpoints WHERE name = ?1", params![final_name], |r| r.get::<_, i64>(0))? > 0 {
                        final_name = format!("{name}-{i}");
                        i += 1;
                    }
                    let update_id = self.max_update_id()?;
                    c.execute(
                        "INSERT INTO checkpoints(uid, name, message, kind, update_id, author_id, author_kind, author_name, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        params![uid, final_name, message, checkpoint_kind, update_id, author.id, author.kind, author.name, created_at],
                    )?;
                    for (doc, state) in docs {
                        let state = B64.decode(state).map_err(|e| Error::Invalid(format!("bad snapshot: {e}")))?;
                        c.execute(
                            "INSERT OR REPLACE INTO checkpoint_docs(checkpoint_uid, doc, state) VALUES (?1, ?2, ?3)",
                            params![uid, doc, state],
                        )?;
                    }
                    1
                }
            }
            Record::Proposal { uid, doc, old_text, new_text, rationale, author, status, created_at, resolved_at, resolved_by, updated_at } => {
                match self.local_updated_at("proposals", "uid", &uid)? {
                    Some(local) if local >= updated_at => 0,
                    Some(_) => c.execute(
                        "UPDATE proposals SET status = ?2, resolved_at = ?3, resolved_by = ?4, updated_at = ?5 WHERE uid = ?1",
                        params![uid, status, resolved_at, resolved_by, updated_at],
                    )?,
                    None => c.execute(
                        "INSERT INTO proposals(uid, doc, old_text, new_text, rationale, author_id, author_kind, author_name, status, created_at, resolved_at, resolved_by, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                        params![uid, doc, old_text, new_text, rationale, author.id, author.kind, author.name, status, created_at, resolved_at, resolved_by, updated_at],
                    )?,
                }
            }
            Record::Round { uid, title, persona, venue, author, status, summary, scores, created_at, finished_at, updated_at } => {
                match self.local_updated_at("review_rounds", "uid", &uid)? {
                    Some(local) if local >= updated_at => 0,
                    Some(_) => c.execute(
                        "UPDATE review_rounds SET status = ?2, summary = ?3, scores = ?4, finished_at = ?5, updated_at = ?6 WHERE uid = ?1",
                        params![uid, status, summary, scores, finished_at, updated_at],
                    )?,
                    None => c.execute(
                        "INSERT INTO review_rounds(uid, title, persona, venue, author_id, author_kind, author_name, update_id, status, summary, scores, created_at, finished_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                        params![uid, title, persona, venue, author.id, author.kind, author.name, self.max_update_id()?, status, summary, scores, created_at, finished_at, updated_at],
                    )?,
                }
            }
            Record::Comment { uid, round_uid, doc, section, quote, offset_hint, severity, category, body, status, response, responder, created_at, updated_at } => {
                let (rid, rkind, rname) = match &responder {
                    Some(a) => (Some(a.id.clone()), Some(a.kind.clone()), Some(a.name.clone())),
                    None => (None, None, None),
                };
                match self.local_updated_at("review_comments", "uid", &uid)? {
                    Some(local) if local >= updated_at => 0,
                    Some(_) => c.execute(
                        "UPDATE review_comments SET status = ?2, response = ?3, responder_id = ?4, responder_kind = ?5, responder_name = ?6, updated_at = ?7 WHERE uid = ?1",
                        params![uid, status, response, rid, rkind, rname, updated_at],
                    )?,
                    None => {
                        let round_id: Option<i64> = c
                            .query_row("SELECT id FROM review_rounds WHERE uid = ?1", params![round_uid], |r| r.get(0))
                            .optional()?;
                        match round_id {
                            None => 0,
                            Some(round_id) => c.execute(
                                "INSERT INTO review_comments(uid, round_id, round_uid, doc, section, quote, offset_hint, severity, category, body, status, response, responder_id, responder_kind, responder_name, created_at, updated_at)
                                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                                params![uid, round_id, round_uid, doc, section, quote, offset_hint, severity, category, body, status, response, rid, rkind, rname, created_at, updated_at],
                            )?,
                        }
                    }
                }
            }
            Record::File { path, content, deleted, updated_at } => match self.local_updated_at("project_files", "path", &path)? {
                Some(local) if local >= updated_at => 0,
                _ => c.execute(
                    "INSERT INTO project_files(path, content, deleted, updated_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(path) DO UPDATE SET content = excluded.content, deleted = excluded.deleted, updated_at = excluded.updated_at",
                    params![path, content, deleted as i64, updated_at],
                )?,
            },
        };
        Ok(n > 0)
    }
}

// ---- shared project files ----------------------------------------------------------

/// Files shared between devices besides documents: the manifest and project skills.
pub fn shared_files_on_disk(project: &Project) -> Vec<String> {
    let mut out = vec![crate::project::MANIFEST.to_string()];
    let mut stack = vec![project.skills_dir()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Some(rel) = project.relative(&p) {
                out.push(rel);
            }
        }
    }
    out.sort();
    out
}

/// Record on-disk changes to shared files (manifest, skills) in the store.
pub fn capture_files(store: &Store, project: &Project) -> Result<usize> {
    let mut changed = 0;
    let on_disk = shared_files_on_disk(project);
    for rel in &on_disk {
        if let Ok(content) = std::fs::read_to_string(project.root.join(rel)) {
            if store.put_file(rel, &content, false)? {
                changed += 1;
            }
        }
    }
    for (path, _, deleted) in store.files()? {
        if !deleted && path.starts_with(".paper/skills/") && !on_disk.contains(&path) && store.put_file(&path, "", true)? {
            changed += 1;
        }
    }
    Ok(changed)
}

/// Write shared files that changed remotely to disk.
pub fn apply_files(store: &Store, root: &std::path::Path, paths: &[String]) -> Result<()> {
    for path in paths {
        let rel = crate::project::normalize_rel(path)?;
        let abs = root.join(&rel);
        match store.file(&rel)? {
            Some((_, true, _)) => {
                let _ = std::fs::remove_file(&abs);
            }
            Some((content, false, _)) if std::fs::read_to_string(&abs).ok().as_deref() != Some(content.as_str()) => {
                if let Some(parent) = abs.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&abs, content)?;
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Author, Format, Ownership, Workspace};

    fn project(title: &str) -> (tempfile::TempDir, Workspace) {
        std::env::set_var("PAPER_NO_CLAUDE_SKILLS", "1");
        let dir = tempfile::tempdir().unwrap();
        let p = Project::init(dir.path(), title, Format::Typst).unwrap();
        let ws = Workspace::open(p, Ownership::Auto).unwrap();
        (dir, ws)
    }

    /// Two devices exchange everything through export/import, like the relay does.
    #[test]
    fn two_devices_converge() {
        let (_a_dir, mut a) = project("Shared");
        a.store().backfill_changelog().unwrap();
        capture_files(a.store(), &a.project).unwrap();
        let me = Author::human("ada");
        let agent = Author::agent("claude-code");
        a.edit("main.typ", "= Method <sec:method>\n", "= Method <sec:method>\nWe prune heads.\n", None, &agent, Some("m")).unwrap();
        let round = a.start_review("R1", Some("methods-skeptic"), None, &agent).unwrap();
        a.add_review_comment(round.id, "main.typ", "We prune heads.", "major", "soundness", "Which heads?", None).unwrap();
        a.checkpoint("v1", "first", "checkpoint", &me).unwrap();

        // Device B: empty store, pulls everything from A.
        let b_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(b_dir.path().join(".paper")).unwrap();
        let b_store = Store::open(&b_dir.path().join(".paper/history.db")).unwrap();
        let batch = a.store().export_changes(0, None, 10_000).unwrap();
        assert!(!batch.more);
        let out = b_store.import_records(batch.records.clone(), "device-a").unwrap();
        assert!(out.files.contains(&"paper.toml".to_string()));
        apply_files(&b_store, b_dir.path(), &out.files).unwrap();
        drop(b_store);
        let mut b = Workspace::open(Project::open(b_dir.path()).unwrap(), Ownership::Auto).unwrap();
        assert_eq!(b.text("main.typ").unwrap(), a.text("main.typ").unwrap());
        assert!(std::fs::read_to_string(b_dir.path().join("main.typ")).unwrap().contains("We prune heads."));
        assert_eq!(b.review_comments(&Default::default()).unwrap().len(), 1);
        // Checkpoint snapshot reconstructs on the other device.
        let diff = b.diff(&crate::history::VersionRef::parse("v1"), &crate::history::VersionRef::Now, None).unwrap();
        assert!(diff.is_empty(), "{diff:?}");
        // Re-importing is a no-op.
        let again = b.store().import_records(batch.records, "device-a").unwrap();
        assert_eq!(again.changed, 0);

        // B responds and edits; A pulls B's changes (excluding what A sent).
        let cursor = b.store().max_seq().unwrap();
        let c = b.review_comments(&Default::default()).unwrap()[0].clone();
        b.respond_to_comment(c.id, "Clarified.", "addressed", &me).unwrap();
        b.edit("main.typ", "We prune heads.", "We prune the 20% least important heads.", None, &me, None).unwrap();
        let back = b.store().export_changes(cursor, Some("device-a"), 10_000).unwrap();
        // The sync client always uses its own connection, like another process.
        let a_sync = Store::open(&a.project.db_path()).unwrap();
        a_sync.import_records(back.records, "device-b").unwrap();
        a.poll().unwrap();
        assert!(a.text("main.typ").unwrap().contains("20% least important"));
        assert_eq!(a.review_comments(&Default::default()).unwrap()[0].status, "addressed");
        // A doesn't send B's changes back to B.
        let echo = a.store().export_changes(0, Some("device-b"), 10_000).unwrap();
        assert!(echo.records.iter().all(|r| !matches!(r, Record::Comment { status, .. } if status == "addressed")));
    }
}
