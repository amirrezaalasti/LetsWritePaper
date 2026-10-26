//! The editing engine shared by the desktop app, the MCP server and the CLI.
//!
//! Many processes may open the same project. They coordinate through the
//! SQLite update log: each process appends its own CRDT updates and tails the
//! others' (`poll`). Exactly one process — the *owner*, holder of
//! `.paper/owner.lock` — mirrors documents to their files on disk and turns
//! on-disk edits made by other tools into attributed CRDT updates.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;

use serde::{Deserialize, Serialize};

use crate::author::Author;
use crate::doc::{BlameRun, PaperDoc};
use crate::history::{diff_doc, group_log, DocDiff, LogEntry, VersionRef};
use crate::outline::{find_section, outline, section_at, Section};
use crate::project::{normalize_rel, Project};
use crate::review::{self, ReviewComment, ReviewRound, Scores};
use crate::skills::{AppliedSkill, SkillRegistry};
use crate::store::{Checkpoint, CommentFilter, NewUpdate, Proposal, Store};
use crate::textdiff::{byte_offset, compute_edits, line_of, utf16_offset};
use crate::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ownership {
    /// Become the owner if no other process is.
    Auto,
    /// Never touch files on disk (read-mostly tools, tests).
    Never,
}

/// Something observers (the app UI) should react to.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceEvent {
    DocUpdate { path: String, update: Vec<u8>, author: Author },
    DocAdded { path: String },
    DocRemoved { path: String },
    /// Checkpoints, proposals, reviews or skills may have changed.
    MetaChanged,
}

/// File contents as last mirrored to disk, and the CRDT state that produced them.
struct Materialized {
    text: String,
    state: Vec<u8>,
}

/// A resolved point in history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    Now,
    /// State after this local update id.
    Upto(i64),
    Checkpoint { uid: String, update_id: i64 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditOutcome {
    pub update_id: Option<i64>,
    /// 1-based line where the edit starts.
    pub line: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlameLine {
    pub line: usize,
    pub text: String,
    /// Author id that wrote most of the line.
    pub author: Option<String>,
    /// Every author id with text on the line, by share.
    pub authors: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillInstructions {
    pub name: String,
    pub reason: String,
    pub instructions: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SectionContext {
    pub path: String,
    pub section: Option<Section>,
    pub text: String,
    pub skills: Vec<SkillInstructions>,
    pub open_comments: Vec<ReviewComment>,
    pub pending_proposals: Vec<Proposal>,
}

pub struct Workspace {
    pub project: Project,
    pub skills: SkillRegistry,
    store: Store,
    origin: String,
    session: String,
    docs: BTreeMap<String, PaperDoc>,
    last_seen: i64,
    data_version: i64,
    lock: Option<File>,
    materialized: HashMap<String, Materialized>,
    /// Per document, the newest history update this process has applied.
    applied: HashMap<String, i64>,
    events: Vec<WorkspaceEvent>,
}

/// What the owner last wrote to each file, persisted in `.paper/mirror.json`,
/// so a later owner can tell a stale file from one edited by another tool.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct MirrorEntry {
    hash: String,
    upto: i64,
}

fn content_hash(text: &str) -> String {
    // FNV-1a: stable across platforms and Rust versions.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}{:x}", text.len())
}

impl Workspace {
    pub fn open(project: Project, ownership: Ownership) -> Result<Self> {
        let store = Store::open(&project.db_path())?;
        let lock = match ownership {
            Ownership::Auto => try_lock(&project),
            Ownership::Never => None,
        };
        let origin = uuid::Uuid::new_v4().to_string();
        let last_seen = store.max_update_id()?;
        let mut docs = BTreeMap::new();
        for path in store.documents()? {
            docs.insert(path.clone(), PaperDoc::from_updates(store.doc_updates(&path, Some(last_seen))?)?);
        }
        let data_version = store.data_version()?;
        let skills = SkillRegistry::load(Some(&project));
        let mut ws = Workspace {
            project,
            skills,
            store,
            session: origin.clone(),
            origin,
            docs,
            last_seen,
            data_version,
            lock,
            materialized: HashMap::new(),
            applied: HashMap::new(),
            events: vec![],
        };
        if ws.is_owner() {
            ws.reconcile_disk()?;
        }
        Ok(ws)
    }

    /// Label for grouping this process's edits in history (e.g. an agent run).
    pub fn set_session(&mut self, session: impl Into<String>) {
        self.session = session.into();
    }

    pub fn is_owner(&self) -> bool {
        self.lock.is_some()
    }

    /// Try to become the owner (e.g. after the previous owner exited).
    pub fn try_take_ownership(&mut self) -> Result<bool> {
        if self.lock.is_none() {
            self.lock = try_lock(&self.project);
            if self.lock.is_some() {
                self.poll()?;
                self.reconcile_disk()?;
            }
        }
        Ok(self.is_owner())
    }

    pub fn drain_events(&mut self) -> Vec<WorkspaceEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn reload_skills(&mut self) {
        self.skills = SkillRegistry::load(Some(&self.project));
        self.events.push(WorkspaceEvent::MetaChanged);
    }

    pub fn reload_manifest(&mut self) -> Result<()> {
        self.project = Project::open(&self.project.root)?;
        self.reload_skills();
        Ok(())
    }

    // ---- reading -----------------------------------------------------------

    pub fn documents(&self) -> Vec<String> {
        self.docs.keys().cloned().collect()
    }

    fn doc(&self, path: &str) -> Result<&PaperDoc> {
        self.docs.get(path).ok_or_else(|| Error::NoDocument(path.into()))
    }

    pub fn text(&self, path: &str) -> Result<String> {
        Ok(self.doc(&normalize_rel(path)?)?.text())
    }

    /// Full CRDT state, for clients that keep their own replica (the editor).
    pub fn doc_state(&self, path: &str) -> Result<Vec<u8>> {
        Ok(self.doc(&normalize_rel(path)?)?.encode_state())
    }

    pub fn outline(&self, path: &str) -> Result<Vec<Section>> {
        let path = normalize_rel(path)?;
        Ok(outline(&path, &self.doc(&path)?.text()))
    }

    /// Text of one section (heading included) and its metadata.
    pub fn section(&self, path: &str, query: &str) -> Result<(Section, String)> {
        let path = normalize_rel(path)?;
        let text = self.doc(&path)?.text();
        let sections = outline(&path, &text);
        let s = find_section(&sections, query).ok_or_else(|| Error::NoSection(query.into()))?.clone();
        let body = text[s.start..s.end].to_string();
        Ok((s, body))
    }

    /// Search every document for a section, preferring the main file.
    pub fn find_section_anywhere(&self, query: &str) -> Option<(String, Section)> {
        let main = self.project.manifest.paper.main.clone();
        let mut order: Vec<String> = vec![main.clone()];
        order.extend(self.documents().into_iter().filter(|d| *d != main));
        order.into_iter().find_map(|p| {
            let sections = self.outline(&p).ok()?;
            find_section(&sections, query).cloned().map(|s| (p, s))
        })
    }

    // ---- writing -----------------------------------------------------------

    fn record(&mut self, path: &str, update: Vec<u8>, author: &Author, message: Option<&str>, before: &str, emit: bool) -> Result<i64> {
        let after = self.doc(path)?.text();
        let (mut added, mut removed) = (0i64, 0i64);
        for e in compute_edits(before, &after) {
            removed += before[e.start..e.end].chars().count() as i64;
            added += e.insert.chars().count() as i64;
        }
        let id = self.store.insert_update(&NewUpdate {
            doc: path,
            author,
            session: &self.session,
            origin: &self.origin,
            message,
            added,
            removed,
            data: &update,
        })?;
        self.applied.insert(path.to_string(), id);
        if emit {
            self.events.push(WorkspaceEvent::DocUpdate { path: path.into(), update, author: author.clone() });
        }
        self.materialize(path)?;
        Ok(id)
    }

    pub fn create_document(&mut self, path: &str, content: &str, author: &Author, message: Option<&str>) -> Result<Option<i64>> {
        let path = normalize_rel(path)?;
        if self.docs.contains_key(&path) {
            return Err(Error::Invalid(format!("document already exists: {path}")));
        }
        self.store.add_document(&path)?;
        let doc = PaperDoc::new();
        let update = doc.set_text(&author.id, content);
        self.docs.insert(path.clone(), doc);
        self.events.push(WorkspaceEvent::DocAdded { path: path.clone() });
        match update {
            Some(u) => self.record(&path, u, author, message.or(Some("created")), "", true).map(Some),
            None => {
                self.materialize(&path)?;
                Ok(None)
            }
        }
    }

    /// Soft-delete: the document leaves the project but stays in history.
    pub fn delete_document(&mut self, path: &str) -> Result<()> {
        let path = normalize_rel(path)?;
        self.doc(&path)?;
        self.store.remove_document(&path)?;
        self.docs.remove(&path);
        self.materialized.remove(&path);
        if self.is_owner() {
            let _ = std::fs::remove_file(self.project.resolve(&path)?);
            self.write_mirror_entry(&path, None);
        }
        self.events.push(WorkspaceEvent::DocRemoved { path });
        Ok(())
    }

    /// Apply an update produced by an editor replica (Yjs in the app).
    pub fn apply_update(&mut self, path: &str, update: &[u8], author: &Author, message: Option<&str>) -> Result<i64> {
        let path = normalize_rel(path)?;
        let doc = self.doc(&path)?;
        let before = doc.text();
        doc.apply_update(update)?;
        self.record(&path, update.to_vec(), author, message, &before, false)
    }

    /// Replace a UTF-16 range of a document.
    pub fn splice(&mut self, path: &str, start: u32, end: u32, insert: &str, author: &Author, message: Option<&str>) -> Result<Option<i64>> {
        let path = normalize_rel(path)?;
        let doc = self.doc(&path)?;
        let before = doc.text();
        let len = doc.len_utf16();
        if start > end || end > len {
            return Err(Error::Invalid(format!("range {start}..{end} out of bounds (length {len})")));
        }
        match doc.splice(&author.id, start, end, insert) {
            Some(u) => self.record(&path, u, author, message, &before, true).map(Some),
            None => Ok(None),
        }
    }

    /// Make a document's text equal `new_text` with minimal edits.
    pub fn set_text(&mut self, path: &str, new_text: &str, author: &Author, message: Option<&str>) -> Result<Option<i64>> {
        let path = normalize_rel(path)?;
        let doc = self.doc(&path)?;
        let before = doc.text();
        match doc.set_text(&author.id, new_text) {
            Some(u) => self.record(&path, u, author, message, &before, true).map(Some),
            None => Ok(None),
        }
    }

    /// Replace an exact excerpt. `occurrence` (1-based) picks among repeats;
    /// without it the excerpt must be unique.
    pub fn edit(&mut self, path: &str, old: &str, new: &str, occurrence: Option<usize>, author: &Author, message: Option<&str>) -> Result<EditOutcome> {
        let path = normalize_rel(path)?;
        let text = self.doc(&path)?.text();
        let at = locate(&path, &text, old, occurrence)?;
        let edits: Vec<_> = compute_edits(old, new)
            .into_iter()
            .map(|e| (utf16_offset(&text, at + e.start), utf16_offset(&text, at + e.end), e.insert))
            .collect();
        let line = line_of(&text, at);
        let update = self.doc(&path)?.apply_edits(&author.id, &edits);
        let update_id = match update {
            Some(u) => Some(self.record(&path, u, author, message, &text, true)?),
            None => None,
        };
        Ok(EditOutcome { update_id, line })
    }

    // ---- multi-process sync ------------------------------------------------

    /// Pull changes made by other processes. Returns true if anything changed.
    pub fn poll(&mut self) -> Result<bool> {
        let version = self.store.data_version()?;
        if version == self.data_version {
            return Ok(false);
        }
        self.data_version = version;
        let max = self.store.max_update_id()?;

        let live = self.store.documents()?;
        for path in &live {
            if !self.docs.contains_key(path) {
                let doc = PaperDoc::from_updates(self.store.doc_updates(path, Some(self.last_seen))?)?;
                self.docs.insert(path.clone(), doc);
                self.events.push(WorkspaceEvent::DocAdded { path: path.clone() });
            }
        }
        let removed: Vec<String> = self.docs.keys().filter(|p| !live.contains(p)).cloned().collect();
        for path in removed {
            self.docs.remove(&path);
            self.materialized.remove(&path);
            if self.is_owner() {
                let _ = std::fs::remove_file(self.project.resolve(&path)?);
            }
            self.events.push(WorkspaceEvent::DocRemoved { path });
        }

        let mut changed = vec![];
        let mut seen = self.last_seen;
        for rec in self.store.updates_since(self.last_seen, &self.origin)? {
            seen = seen.max(rec.meta.id);
            let Some(doc) = self.docs.get(&rec.meta.doc) else { continue };
            doc.apply_update(&rec.data)?;
            self.applied.insert(rec.meta.doc.clone(), rec.meta.id);
            if !changed.contains(&rec.meta.doc) {
                changed.push(rec.meta.doc.clone());
            }
            self.events.push(WorkspaceEvent::DocUpdate { path: rec.meta.doc, update: rec.data, author: rec.meta.author });
        }
        self.last_seen = seen.max(max);
        for path in changed {
            self.materialize(&path)?;
        }
        self.events.push(WorkspaceEvent::MetaChanged);
        Ok(true)
    }

    // ---- files on disk -----------------------------------------------------

    /// Owner only: mirror a document to its file.
    fn materialize(&mut self, path: &str) -> Result<()> {
        if !self.is_owner() {
            return Ok(());
        }
        let doc = self.doc(path)?;
        let text = doc.text();
        let state = doc.encode_state();
        let abs = self.project.resolve(path)?;
        if std::fs::read_to_string(&abs).ok().as_deref() != Some(text.as_str()) {
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&abs, &text)?;
        }
        let entry = MirrorEntry { hash: content_hash(&text), upto: self.applied_upto(path) };
        self.write_mirror_entry(path, Some(entry));
        self.materialized.insert(path.to_string(), Materialized { text, state });
        Ok(())
    }

    fn applied_upto(&self, path: &str) -> i64 {
        self.applied.get(path).copied().unwrap_or(self.last_seen)
    }

    fn mirror_path(&self) -> std::path::PathBuf {
        self.project.state_dir().join("mirror.json")
    }

    fn read_mirror(&self) -> HashMap<String, MirrorEntry> {
        std::fs::read_to_string(self.mirror_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn write_mirror_entry(&self, path: &str, entry: Option<MirrorEntry>) {
        let mut all = self.read_mirror();
        let changed = match &entry {
            Some(e) => all.get(path).map(|old| old.hash != e.hash).unwrap_or(true),
            None => all.contains_key(path),
        };
        if !changed {
            return;
        }
        match entry {
            Some(e) => all.insert(path.to_string(), e),
            None => all.remove(path),
        };
        if let Ok(json) = serde_json::to_string(&all) {
            let _ = std::fs::write(self.mirror_path(), json);
        }
    }

    /// Owner start-up: adopt edits made while nobody was watching, import new
    /// files, and restore files missing from disk.
    fn reconcile_disk(&mut self) -> Result<()> {
        for path in self.project.scan_files() {
            if !self.docs.contains_key(&path) {
                if let Ok(text) = std::fs::read_to_string(self.project.resolve(&path)?) {
                    self.create_document(&path, &text, &Author::external(), Some("imported from disk"))?;
                }
            }
        }
        let mirror = self.read_mirror();
        for path in self.documents() {
            let abs = self.project.resolve(&path)?;
            match std::fs::read_to_string(&abs) {
                Ok(disk) if disk != self.doc(&path)?.text() => match mirror.get(&path) {
                    // The file is exactly what an earlier owner wrote: it is
                    // merely behind the history, not edited. Bring it up to date.
                    Some(m) if m.hash == content_hash(&disk) => self.materialize(&path)?,
                    // Edited outside since it was last written: merge the edit
                    // relative to what was written, so newer history is kept.
                    Some(m) => {
                        let base = PaperDoc::from_updates(self.store.doc_updates(&path, Some(m.upto))?)?;
                        self.materialized.insert(path.clone(), Materialized { text: base.text(), state: base.encode_state() });
                        self.ingest_text(&path, &disk, &Author::external(), Some("edited outside LetsWritePaper"))?;
                    }
                    None => {
                        self.ingest_text(&path, &disk, &Author::external(), Some("edited outside LetsWritePaper"))?;
                    }
                },
                _ => self.materialize(&path)?,
            }
        }
        Ok(())
    }

    /// Owner: react to a file-system event for `path` (project-relative).
    /// Returns true if the change was adopted into history.
    pub fn check_disk(&mut self, path: &str) -> Result<bool> {
        if !self.is_owner() {
            return Ok(false);
        }
        // Another process may already have recorded this change (e.g. a hook).
        self.poll()?;
        let path = normalize_rel(path)?;
        if !self.project.is_tracked_path(&path) {
            return Ok(false);
        }
        let abs = self.project.resolve(&path)?;
        if !abs.exists() {
            if self.docs.contains_key(&path) {
                self.delete_document(&path)?;
                return Ok(true);
            }
            return Ok(false);
        }
        let Ok(disk) = std::fs::read_to_string(&abs) else { return Ok(false) };
        if !self.docs.contains_key(&path) {
            self.create_document(&path, &disk, &Author::external(), Some("new file on disk"))?;
            return Ok(true);
        }
        let current = self.doc(&path)?.text();
        if disk == current {
            self.materialize(&path)?;
            return Ok(false);
        }
        if self.materialized.get(&path).is_some_and(|m| m.text == disk) {
            // The file is just behind the CRDT; bring it up to date.
            self.materialize(&path)?;
            return Ok(false);
        }
        self.ingest_text(&path, &disk, &Author::external(), Some("edited outside LetsWritePaper"))?;
        Ok(true)
    }

    /// Adopt the file's current contents as an edit by `author` (used by the
    /// `paper ingest` hook so agents editing files directly get credit).
    pub fn ingest_file(&mut self, path: &str, author: &Author, message: Option<&str>) -> Result<Option<i64>> {
        self.poll()?;
        let path = normalize_rel(path)?;
        let disk = std::fs::read_to_string(self.project.resolve(&path)?)?;
        if !self.docs.contains_key(&path) {
            return self.create_document(&path, &disk, author, message);
        }
        self.ingest_text(&path, &disk, author, message)
    }

    /// Merge a new on-disk version. When the CRDT has moved on since the file
    /// was last written, the file's edits are computed against what it was
    /// written from (a shadow replica) so concurrent edits are not reverted.
    fn ingest_text(&mut self, path: &str, disk: &str, author: &Author, message: Option<&str>) -> Result<Option<i64>> {
        let before = self.doc(path)?.text();
        let update = match self.materialized.get(path) {
            Some(m) if m.text != before => {
                let shadow = PaperDoc::from_updates([&m.state])?;
                let u = shadow.set_text(&author.id, disk);
                if let Some(u) = &u {
                    self.doc(path)?.apply_update(u)?;
                }
                u
            }
            _ => self.doc(path)?.set_text(&author.id, disk),
        };
        match update {
            Some(u) => self.record(path, u, author, message, &before, true).map(Some),
            None => Ok(None),
        }
    }

    /// Non-owners: wait (briefly) until the owner has mirrored the current
    /// state to disk, so that e.g. a git commit captures it.
    pub fn wait_for_disk(&mut self) -> Result<()> {
        if self.is_owner() {
            for p in self.documents() {
                self.materialize(&p)?;
            }
            return Ok(());
        }
        for _ in 0..40 {
            let synced = self.docs.iter().all(|(p, d)| {
                self.project.resolve(p).ok().and_then(|a| std::fs::read_to_string(a).ok()).as_deref() == Some(d.text().as_str())
            });
            if synced {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        Ok(())
    }

    // ---- history -----------------------------------------------------------

    pub fn log(&self, path: Option<&str>, limit: usize) -> Result<Vec<LogEntry>> {
        let path = path.map(normalize_rel).transpose()?;
        let metas = self.store.log(path.as_deref(), None, limit.saturating_mul(50).max(200))?;
        let mut grouped = group_log(&metas);
        grouped.truncate(limit);
        Ok(grouped)
    }

    /// What a version reference denotes.
    pub fn resolve_version(&self, v: &VersionRef) -> Result<Resolved> {
        let cp = |c: Checkpoint| Resolved::Checkpoint { uid: c.uid, update_id: c.update_id };
        Ok(match v {
            VersionRef::Now => Resolved::Now,
            VersionRef::Beginning => Resolved::Upto(0),
            VersionRef::Update(id) => Resolved::Upto(*id),
            VersionRef::LastCheckpoint => cp(self
                .store
                .checkpoints()?
                .into_iter()
                .next()
                .ok_or_else(|| Error::NotFound("no checkpoints yet".into()))?),
            VersionRef::Checkpoint(name) => cp(self
                .store
                .checkpoint(name)?
                .ok_or_else(|| Error::NotFound(format!("checkpoint {name:?}")))?),
        })
    }

    /// Document text at a version (None when the document did not exist).
    pub fn text_at(&self, path: &str, version: &Resolved) -> Result<Option<String>> {
        let path = normalize_rel(path)?;
        match version {
            Resolved::Now => Ok(self.docs.get(&path).map(|d| d.text())),
            Resolved::Checkpoint { uid, update_id } => {
                // Snapshots make checkpoints device-independent; older
                // checkpoints fall back to the local log position.
                if self.store.checkpoint_has_snapshots(uid)? {
                    return match self.store.checkpoint_snapshot(uid, &path)? {
                        Some(state) => Ok(Some(PaperDoc::from_updates([state])?.text())),
                        None => Ok(None),
                    };
                }
                self.text_at(&path, &Resolved::Upto(*update_id))
            }
            Resolved::Upto(id) => {
                let updates = self.store.doc_updates(&path, Some(*id))?;
                if updates.is_empty() {
                    return Ok(None);
                }
                Ok(Some(PaperDoc::from_updates(updates)?.text()))
            }
        }
    }

    pub fn diff(&self, from: &VersionRef, to: &VersionRef, path: Option<&str>) -> Result<Vec<DocDiff>> {
        let (a, b) = (self.resolve_version(from)?, self.resolve_version(to)?);
        let paths = match path {
            Some(p) => vec![normalize_rel(p)?],
            None => {
                let mut all = self.store.all_doc_paths()?;
                all.extend(self.documents());
                all.sort();
                all.dedup();
                all
            }
        };
        let mut out = vec![];
        for p in paths {
            let old = self.text_at(&p, &a)?;
            let new = self.text_at(&p, &b)?;
            out.extend(diff_doc(&p, old.as_deref(), new.as_deref()));
        }
        Ok(out)
    }

    pub fn blame(&self, path: &str) -> Result<Vec<BlameLine>> {
        let path = normalize_rel(path)?;
        Ok(blame_lines(&self.doc(&path)?.blame_runs()))
    }

    pub fn authors(&self) -> Result<HashMap<String, Author>> {
        self.store.authors()
    }

    /// Record a named version. `kind` is `checkpoint` or a milestone name
    /// (`submitted`, `rebuttal`, `camera-ready`, `arxiv`, ...). Also commits to
    /// git when the project is a repository.
    pub fn checkpoint(&mut self, name: &str, message: &str, kind: &str, author: &Author) -> Result<Checkpoint> {
        self.poll()?;
        let upto = self.store.max_update_id()?;
        let previous = self.store.checkpoints()?.first().map(|c| c.update_id).unwrap_or(0);
        let snapshots: Vec<(String, Vec<u8>)> = self.docs.iter().map(|(p, d)| (p.clone(), d.encode_state())).collect();
        let mut cp = self.store.insert_checkpoint(name, message, kind, upto, author, &snapshots)?;
        self.wait_for_disk()?;
        let mut contributors = self.store.authors_between(previous, upto)?;
        contributors.retain(|a| a.id != author.id);
        let subject = if kind == "checkpoint" { name.to_string() } else { format!("[{kind}] {name}") };
        if let Some(sha) = crate::git::commit_all(&self.project.root, &subject, message, &contributors) {
            self.store.set_checkpoint_commit(cp.id, &sha)?;
            cp.git_commit = Some(sha);
        }
        self.events.push(WorkspaceEvent::MetaChanged);
        Ok(cp)
    }

    pub fn checkpoints(&self) -> Result<Vec<Checkpoint>> {
        self.store.checkpoints()
    }

    /// Bring a document back to its text at a version, as a new attributed edit
    /// (history is never rewritten).
    pub fn restore(&mut self, path: &str, version: &VersionRef, author: &Author) -> Result<Option<i64>> {
        let resolved = self.resolve_version(version)?;
        let text = self
            .text_at(path, &resolved)?
            .ok_or_else(|| Error::NotFound(format!("{path} did not exist at that version")))?;
        let msg = format!("restored to {version:?}");
        if self.docs.contains_key(&normalize_rel(path)?) {
            self.set_text(path, &text, author, Some(&msg))
        } else {
            self.create_document(path, &text, author, Some(&msg))
        }
    }

    // ---- proposals (suggested edits) ---------------------------------------

    pub fn propose(&mut self, path: &str, old: &str, new: &str, rationale: &str, author: &Author) -> Result<Proposal> {
        let path = normalize_rel(path)?;
        let text = self.doc(&path)?.text();
        locate(&path, &text, old, Some(1))?;
        let p = self.store.insert_proposal(&path, old, new, rationale, author)?;
        self.events.push(WorkspaceEvent::MetaChanged);
        Ok(p)
    }

    /// Proposals, with pending ones whose text has disappeared marked `stale`.
    pub fn proposals(&self, status: Option<&str>) -> Result<Vec<Proposal>> {
        let mut list = self.store.proposals(if status == Some("stale") { Some("pending") } else { status })?;
        for p in &mut list {
            if p.status == "pending" {
                let found = self.docs.get(&p.doc).map(|d| d.text().contains(&p.old_text)).unwrap_or(false);
                if !found {
                    p.status = "stale".into();
                }
            }
        }
        if status == Some("stale") {
            list.retain(|p| p.status == "stale");
        }
        Ok(list)
    }

    /// Apply a pending proposal; the edit is credited to whoever proposed it.
    pub fn accept_proposal(&mut self, id: i64, by: &Author) -> Result<EditOutcome> {
        let p = self.store.proposal(id)?;
        if p.status != "pending" {
            return Err(Error::Invalid(format!("proposal #{id} is {}", p.status)));
        }
        let msg = format!("proposal #{id} accepted by {}: {}", by.name, p.rationale);
        let out = self.edit(&p.doc, &p.old_text, &p.new_text, Some(1), &p.author, Some(&msg))?;
        self.store.resolve_proposal(id, "accepted", by)?;
        self.events.push(WorkspaceEvent::MetaChanged);
        Ok(out)
    }

    pub fn reject_proposal(&mut self, id: i64, by: &Author) -> Result<()> {
        self.store.resolve_proposal(id, "rejected", by)?;
        self.events.push(WorkspaceEvent::MetaChanged);
        Ok(())
    }

    // ---- peer review ---------------------------------------------------------

    pub fn start_review(&mut self, title: &str, persona: Option<&str>, venue: Option<&str>, reviewer: &Author) -> Result<ReviewRound> {
        self.poll()?;
        let upto = self.store.max_update_id()?;
        let venue = venue.or(self.project.manifest.paper.venue.as_deref());
        let r = self.store.insert_round(title, persona, venue, reviewer, upto)?;
        self.events.push(WorkspaceEvent::MetaChanged);
        Ok(r)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_review_comment(
        &mut self,
        round_id: i64,
        path: &str,
        quote: &str,
        severity: &str,
        category: &str,
        body: &str,
        section: Option<&str>,
    ) -> Result<ReviewComment> {
        review::validate_severity(severity)?;
        review::validate_category(category)?;
        let round = self.store.round(round_id)?;
        if round.status != "open" {
            return Err(Error::Invalid(format!("review round #{round_id} is already finished")));
        }
        let path = normalize_rel(path)?;
        let text = self.doc(&path)?.text();
        let at = locate(&path, &text, quote, Some(1))?;
        let section = match section {
            Some(s) => Some(s.to_string()),
            None => section_at(&outline(&path, &text), at).map(|s| s.title.clone()),
        };
        let hint = utf16_offset(&text, at) as i64;
        let mut c = self
            .store
            .insert_comment(round_id, &path, section.as_deref(), quote, Some(hint), severity, category, body)?;
        c.current_line = Some(line_of(&text, at));
        self.events.push(WorkspaceEvent::MetaChanged);
        Ok(c)
    }

    pub fn finish_review(&mut self, round_id: i64, summary: &str, scores: Option<&Scores>) -> Result<ReviewRound> {
        let r = self.store.finish_round(round_id, summary, scores)?;
        self.events.push(WorkspaceEvent::MetaChanged);
        Ok(r)
    }

    pub fn review_rounds(&self) -> Result<Vec<ReviewRound>> {
        self.store.rounds()
    }

    /// Comments with their quotes re-located in the current text.
    pub fn review_comments(&self, filter: &CommentFilter) -> Result<Vec<ReviewComment>> {
        let mut list = self.store.comments(filter)?;
        for c in &mut list {
            c.current_line = self.docs.get(&c.doc).and_then(|d| {
                let text = d.text();
                let hint = c.offset_hint.map(|h| byte_offset(&text, h as u32)).unwrap_or(0);
                text.match_indices(&c.quote)
                    .map(|(i, _)| i)
                    .min_by_key(|i| i.abs_diff(hint))
                    .map(|i| line_of(&text, i))
            });
        }
        Ok(list)
    }

    pub fn respond_to_comment(&mut self, id: i64, response: &str, status: &str, by: &Author) -> Result<ReviewComment> {
        review::validate_status(status)?;
        let c = self.store.respond_comment(id, response, status, by)?;
        self.events.push(WorkspaceEvent::MetaChanged);
        Ok(c)
    }

    pub fn response_letter(&self) -> Result<String> {
        let mut rounds = vec![];
        for r in self.store.rounds()?.into_iter().rev() {
            let comments = self.store.comments(&CommentFilter { round_id: Some(r.id), ..Default::default() })?;
            rounds.push((r, comments));
        }
        Ok(review::render_response_letter(&self.project.manifest.paper.title, &rounds))
    }

    // ---- skills --------------------------------------------------------------

    pub fn skills_for(&self, section: Option<&str>) -> Vec<AppliedSkill> {
        self.skills.for_section(&self.project.manifest.skills, section)
    }

    /// Everything an agent needs before editing a section: its text, the
    /// skills that apply, and open review feedback and suggestions on it.
    pub fn section_context(&self, path: Option<&str>, section: Option<&str>) -> Result<SectionContext> {
        let (path, sec, text) = match (path, section) {
            (Some(p), Some(q)) => {
                let (s, t) = self.section(p, q)?;
                (normalize_rel(p)?, Some(s), t)
            }
            (None, Some(q)) => {
                let (p, s) = self.find_section_anywhere(q).ok_or_else(|| Error::NoSection(q.into()))?;
                let full = self.text(&p)?;
                let t = full[s.start..s.end].to_string();
                (p, Some(s), t)
            }
            (Some(p), None) => (normalize_rel(p)?, None, self.text(p)?),
            (None, None) => {
                let p = self.project.manifest.paper.main.clone();
                let t = self.text(&p)?;
                (p, None, t)
            }
        };
        let title = sec.as_ref().map(|s| s.title.clone());
        let skills = self
            .skills_for(title.as_deref())
            .into_iter()
            .map(|a| SkillInstructions {
                instructions: self.skills.get(&a.name).map(|s| s.body.clone()).unwrap_or_default(),
                name: a.name,
                reason: a.reason,
            })
            .collect();
        let in_section = |line: Option<usize>| match (&sec, line) {
            (Some(s), Some(l)) => l >= s.line && l <= s.end_line,
            (None, _) => true,
            _ => false,
        };
        let open_comments = self
            .review_comments(&CommentFilter { status: Some("open".into()), doc: Some(path.clone()), ..Default::default() })?
            .into_iter()
            .filter(|c| in_section(c.current_line))
            .collect();
        let pending_proposals = self
            .proposals(Some("pending"))?
            .into_iter()
            .filter(|p| p.doc == path && text.contains(&p.old_text))
            .collect();
        Ok(SectionContext { path, section: sec, text, skills, open_comments, pending_proposals })
    }

    /// Bind a skill to sections matching `pattern` (or globally when `None`) in paper.toml.
    pub fn bind_skill(&mut self, pattern: Option<&str>, skill: &str) -> Result<()> {
        let cfg = &mut self.project.manifest.skills;
        let list = match pattern {
            Some(p) => cfg.sections.entry(p.to_lowercase()).or_default(),
            None => &mut cfg.global,
        };
        if !list.iter().any(|s| s == skill) {
            list.push(skill.to_string());
        }
        cfg.disabled.retain(|s| s != skill);
        self.project.save_manifest()?;
        self.events.push(WorkspaceEvent::MetaChanged);
        Ok(())
    }

    pub fn unbind_skill(&mut self, pattern: Option<&str>, skill: &str) -> Result<()> {
        let cfg = &mut self.project.manifest.skills;
        match pattern {
            Some(p) => {
                let key = p.to_lowercase();
                if let Some(list) = cfg.sections.get_mut(&key) {
                    list.retain(|s| s != skill);
                    if list.is_empty() {
                        cfg.sections.remove(&key);
                    }
                }
            }
            None => cfg.global.retain(|s| s != skill),
        }
        // Also stop automatic matching so the skill really goes away.
        if !cfg.disabled.iter().any(|s| s == skill) {
            cfg.disabled.push(skill.to_string());
        }
        self.project.save_manifest()?;
        self.events.push(WorkspaceEvent::MetaChanged);
        Ok(())
    }
}

fn try_lock(project: &Project) -> Option<File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(project.state_dir().join("owner.lock"))
        .ok()?;
    file.try_lock().ok().map(|_| file)
}

/// Byte offset of the chosen occurrence of `needle` in `text`.
fn locate(path: &str, text: &str, needle: &str, occurrence: Option<usize>) -> Result<usize> {
    if needle.is_empty() {
        return Err(Error::Invalid("old text must not be empty; to insert, include neighbouring text in old_text and new_text".into()));
    }
    let hits: Vec<usize> = text.match_indices(needle).map(|(i, _)| i).collect();
    let snippet = || needle.chars().take(80).collect::<String>();
    match (hits.len(), occurrence) {
        (0, _) => Err(Error::TextNotFound { path: path.into(), snippet: snippet() }),
        (1, _) => Ok(hits[0]),
        (n, None) => Err(Error::AmbiguousText { path: path.into(), count: n }),
        (n, Some(k)) => hits
            .get(k.saturating_sub(1))
            .copied()
            .ok_or_else(|| Error::Invalid(format!("occurrence {k} requested but the text occurs {n} times"))),
    }
}

fn blame_lines(runs: &[BlameRun]) -> Vec<BlameLine> {
    let mut lines: Vec<BlameLine> = vec![];
    let mut text = String::new();
    let mut counts: Vec<(Option<String>, usize)> = vec![];
    let flush = |text: &mut String, counts: &mut Vec<(Option<String>, usize)>, lines: &mut Vec<BlameLine>| {
        counts.sort_by_key(|c| std::cmp::Reverse(c.1));
        let authors: Vec<String> = counts.iter().filter_map(|(a, _)| a.clone()).collect();
        lines.push(BlameLine {
            line: lines.len() + 1,
            text: std::mem::take(text),
            author: counts.first().and_then(|(a, _)| a.clone()),
            authors,
        });
        counts.clear();
    };
    for run in runs {
        for piece in run.text.split_inclusive('\n') {
            let content = piece.trim_end_matches('\n');
            let weight = content.chars().filter(|c| !c.is_whitespace()).count();
            if weight > 0 {
                match counts.iter_mut().find(|(a, _)| *a == run.author) {
                    Some((_, n)) => *n += weight,
                    None => counts.push((run.author.clone(), weight)),
                }
            }
            text.push_str(content);
            if piece.ends_with('\n') {
                flush(&mut text, &mut counts, &mut lines);
            }
        }
    }
    if !text.is_empty() {
        flush(&mut text, &mut counts, &mut lines);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Format;

    fn setup() -> (tempfile::TempDir, Workspace) {
        std::env::set_var("PAPER_NO_CLAUDE_SKILLS", "1");
        let dir = tempfile::tempdir().unwrap();
        let project = Project::init(dir.path(), "Test Paper", Format::Latex).unwrap();
        let ws = Workspace::open(project, Ownership::Auto).unwrap();
        (dir, ws)
    }

    #[test]
    fn imports_and_edits_with_attribution() {
        let (_dir, mut ws) = setup();
        assert!(ws.is_owner());
        assert!(ws.documents().contains(&"main.tex".to_string()));
        let agent = Author::agent("claude-code");
        let out = ws
            .edit("main.tex", "\\section{Introduction}\n", "\\section{Introduction}\nLarge models are expensive.\n", None, &agent, Some("draft intro"))
            .unwrap();
        assert!(out.update_id.is_some());
        let disk = std::fs::read_to_string(ws.project.root.join("main.tex")).unwrap();
        assert!(disk.contains("Large models are expensive."));
        let blame = ws.blame("main.tex").unwrap();
        let line = blame.iter().find(|l| l.text == "Large models are expensive.").unwrap();
        assert_eq!(line.author.as_deref(), Some("agent:claude-code"));
        let log = ws.log(None, 10).unwrap();
        assert_eq!(log[0].author.id, "agent:claude-code");
        assert_eq!(log[0].messages, ["draft intro"]);
    }

    #[test]
    fn two_processes_sync_through_the_log() {
        let (_dir, mut app) = setup();
        let mut agent_proc = Workspace::open(Project::open(&app.project.root).unwrap(), Ownership::Auto).unwrap();
        assert!(!agent_proc.is_owner(), "app holds the owner lock");
        agent_proc
            .edit("main.tex", "\\section{Method}\n", "\\section{Method}\nWe fine-tune.\n", None, &Author::agent("claude"), None)
            .unwrap();
        app.splice("main.tex", 0, 0, "% draft\n", &Author::human("ada"), None).unwrap();
        assert!(app.poll().unwrap());
        assert!(agent_proc.poll().unwrap());
        assert_eq!(app.text("main.tex").unwrap(), agent_proc.text("main.tex").unwrap());
        let disk = std::fs::read_to_string(app.project.root.join("main.tex")).unwrap();
        assert!(disk.starts_with("% draft\n") && disk.contains("We fine-tune."));
        assert!(app.drain_events().iter().any(|e| matches!(e, WorkspaceEvent::DocUpdate { .. })));
    }

    #[test]
    fn external_edit_merges_with_unmirrored_changes() {
        let (_dir, mut ws) = setup();
        let abs = ws.project.root.join("main.tex");
        // Someone edits the file on disk...
        let disk = std::fs::read_to_string(&abs).unwrap().replace("\\section{Method}", "\\section{Approach}");
        std::fs::write(&abs, &disk).unwrap();
        assert!(ws.check_disk("main.tex").unwrap());
        let text = ws.text("main.tex").unwrap();
        assert!(text.contains("\\section{Approach}"));
        let log = ws.log(Some("main.tex"), 1).unwrap();
        assert_eq!(log[0].author.kind, crate::AuthorKind::External);
        // No-op when the file matches.
        assert!(!ws.check_disk("main.tex").unwrap());
    }

    #[test]
    fn stale_file_is_refreshed_not_treated_as_an_edit() {
        let (_dir, owner) = setup();
        let root = owner.project.root.clone();
        // A non-owner records an edit while the owner is gone before mirroring it.
        drop(owner);
        let mut writer = Workspace::open(Project::open(&root).unwrap(), Ownership::Never).unwrap();
        writer
            .edit("main.tex", "\\section{Method}\n", "\\section{Method}\nUnmirrored sentence.\n", None, &Author::agent("claude"), None)
            .unwrap();
        drop(writer);
        assert!(!std::fs::read_to_string(root.join("main.tex")).unwrap().contains("Unmirrored"));
        // The next owner must bring the file up to date, not revert the edit.
        let ws = Workspace::open(Project::open(&root).unwrap(), Ownership::Auto).unwrap();
        assert!(ws.text("main.tex").unwrap().contains("Unmirrored sentence."));
        assert!(std::fs::read_to_string(root.join("main.tex")).unwrap().contains("Unmirrored sentence."));
        assert_eq!(ws.log(Some("main.tex"), 1).unwrap()[0].author.id, "agent:claude");
        drop(ws);

        // Now a real outside edit plus an unmirrored history edit: both survive.
        let mut writer = Workspace::open(Project::open(&root).unwrap(), Ownership::Never).unwrap();
        writer.edit("main.tex", "Unmirrored sentence.", "Unmirrored sentence, revised.", None, &Author::agent("claude"), None).unwrap();
        drop(writer);
        let disk = std::fs::read_to_string(root.join("main.tex")).unwrap().replace("\\section{Experiments}", "\\section{Evaluation}");
        std::fs::write(root.join("main.tex"), disk).unwrap();
        let ws = Workspace::open(Project::open(&root).unwrap(), Ownership::Auto).unwrap();
        let text = ws.text("main.tex").unwrap();
        assert!(text.contains("Unmirrored sentence, revised."), "{text}");
        assert!(text.contains("\\section{Evaluation}"), "{text}");
    }

    #[test]
    fn checkpoints_diffs_and_restore() {
        let (_dir, mut ws) = setup();
        let me = Author::human("ada");
        ws.checkpoint("draft-1", "first draft", "checkpoint", &me).unwrap();
        ws.edit("main.tex", "\\section{Experiments}\n", "\\section{Experiments}\nAccuracy improves by 3 points.\n", None, &me, None)
            .unwrap();
        let diffs = ws.diff(&VersionRef::parse("draft-1"), &VersionRef::Now, None).unwrap();
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].sections[0].section.as_deref(), Some("Experiments"));
        ws.restore("main.tex", &VersionRef::parse("draft-1"), &me).unwrap();
        assert!(!ws.text("main.tex").unwrap().contains("Accuracy improves"));
        assert!(ws.checkpoint("draft-1", "dup", "checkpoint", &me).is_err());
    }

    #[test]
    fn proposals_and_reviews() {
        let (_dir, mut ws) = setup();
        let agent = Author::agent("claude");
        let me = Author::human("ada");
        let p = ws.propose("main.tex", "\\section{Method}", "\\section{Our Method}", "clearer", &agent).unwrap();
        assert_eq!(ws.proposals(Some("pending")).unwrap().len(), 1);
        ws.accept_proposal(p.id, &me).unwrap();
        assert!(ws.text("main.tex").unwrap().contains("Our Method"));
        assert_eq!(ws.log(None, 1).unwrap()[0].author.id, "agent:claude");

        let round = ws.start_review("Mock review", Some("methods-skeptic"), Some("NeurIPS"), &agent).unwrap();
        let c = ws
            .add_review_comment(round.id, "main.tex", "\\section{Our Method}", "major", "soundness", "Method is empty.", None)
            .unwrap();
        assert_eq!(c.section.as_deref(), Some("Our Method"));
        assert!(ws.add_review_comment(round.id, "main.tex", "nope", "major", "soundness", "x", None).is_err());
        assert!(ws.add_review_comment(round.id, "main.tex", "\\section{Our Method}", "fatal", "soundness", "x", None).is_err());
        let ctx = ws.section_context(None, Some("our method")).unwrap();
        assert_eq!(ctx.open_comments.len(), 1);
        assert!(ctx.skills.iter().any(|s| s.name == "paper-methods"));
        assert!(ctx.skills.iter().any(|s| s.name == "latex-hygiene"));
        ws.finish_review(round.id, "Promising but thin.", Some(&Scores { overall: Some(4), ..Default::default() })).unwrap();
        ws.respond_to_comment(c.id, "Added a full method section.", "addressed", &me).unwrap();
        let letter = ws.response_letter().unwrap();
        assert!(letter.contains("Added a full method section."));
    }

    #[test]
    fn skill_bindings_persist() {
        let (_dir, mut ws) = setup();
        ws.bind_skill(Some("Related*"), "my-style").unwrap();
        let reopened = Project::open(&ws.project.root).unwrap();
        assert_eq!(reopened.manifest.skills.sections["related*"], ["my-style"]);
        ws.unbind_skill(None, "paper-introduction").unwrap();
        assert!(!ws.skills_for(Some("Introduction")).iter().any(|s| s.name == "paper-introduction"));
    }
}
