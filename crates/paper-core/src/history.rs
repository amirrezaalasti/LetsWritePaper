//! Paper-aware views over the update log: grouped activity, version
//! references, and section-aware word diffs.

use serde::{Deserialize, Serialize};

use crate::author::Author;
use crate::outline::{outline, section_at};
use crate::store::UpdateMeta;
use crate::textdiff::{word_hunks, word_stats, Hunk};

/// Consecutive updates by the same author to the same document within this
/// window are shown as one entry (one "edit session").
pub const GROUP_WINDOW_MS: i64 = 5 * 60 * 1000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogEntry {
    pub first_id: i64,
    pub last_id: i64,
    pub doc: String,
    pub author: Author,
    pub session: String,
    pub messages: Vec<String>,
    pub added: i64,
    pub removed: i64,
    pub updates: usize,
    pub started_at: i64,
    pub ended_at: i64,
}

/// Group newest-first update metadata into edit sessions (newest first).
pub fn group_log(metas: &[UpdateMeta]) -> Vec<LogEntry> {
    let mut out: Vec<LogEntry> = vec![];
    for m in metas {
        if let Some(e) = out.last_mut() {
            let same = e.doc == m.doc && e.author.id == m.author.id && e.session == m.session;
            if same && e.started_at - m.created_at <= GROUP_WINDOW_MS {
                e.first_id = m.id;
                e.started_at = m.created_at;
                e.added += m.added;
                e.removed += m.removed;
                e.updates += 1;
                if let Some(msg) = &m.message {
                    if !e.messages.contains(msg) {
                        e.messages.insert(0, msg.clone());
                    }
                }
                continue;
            }
        }
        out.push(LogEntry {
            first_id: m.id,
            last_id: m.id,
            doc: m.doc.clone(),
            author: m.author.clone(),
            session: m.session.clone(),
            messages: m.message.iter().cloned().collect(),
            added: m.added,
            removed: m.removed,
            updates: 1,
            started_at: m.created_at,
            ended_at: m.created_at,
        });
    }
    out
}

/// A point in history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VersionRef {
    /// The current state.
    Now,
    /// State after update `id`.
    Update(i64),
    /// A named checkpoint or milestone.
    Checkpoint(String),
    /// The most recent checkpoint.
    LastCheckpoint,
    /// Empty project.
    Beginning,
}

impl VersionRef {
    /// `now`/`HEAD`, `#123` or `@123` (update id), `last`, `start`, or a checkpoint name.
    pub fn parse(s: &str) -> Self {
        let t = s.trim();
        match t.to_lowercase().as_str() {
            "" | "now" | "head" | "current" => return VersionRef::Now,
            "last" | "last-checkpoint" => return VersionRef::LastCheckpoint,
            "start" | "beginning" | "empty" => return VersionRef::Beginning,
            _ => {}
        }
        if let Some(n) = t.strip_prefix(['#', '@']).and_then(|n| n.parse().ok()) {
            return VersionRef::Update(n);
        }
        VersionRef::Checkpoint(t.to_string())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SectionChange {
    /// Section the change falls in (in the newer version); None for preamble.
    pub section: Option<String>,
    pub hunks: Vec<Hunk>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocDiff {
    pub path: String,
    /// `added`, `deleted` or `modified`.
    pub status: String,
    pub words_added: usize,
    pub words_removed: usize,
    pub sections: Vec<SectionChange>,
}

/// Word-level diff of one document, grouped by the section each hunk touches.
pub fn diff_doc(path: &str, old: Option<&str>, new: Option<&str>) -> Option<DocDiff> {
    let status = match (old, new) {
        (None, None) => return None,
        (None, Some(_)) => "added",
        (Some(_), None) => "deleted",
        (Some(a), Some(b)) if a == b => return None,
        _ => "modified",
    };
    let (old, new) = (old.unwrap_or(""), new.unwrap_or(""));
    let (words_added, words_removed) = word_stats(old, new);
    let new_outline = outline(path, new);
    let old_outline = outline(path, old);
    let new_line_starts: Vec<usize> = std::iter::once(0)
        .chain(new.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let old_line_starts: Vec<usize> = std::iter::once(0)
        .chain(old.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let mut sections: Vec<SectionChange> = vec![];
    for hunk in word_hunks(old, new, 1) {
        // Locate the first changed segment's line, not the context line.
        let lead: usize = hunk
            .segments
            .iter()
            .take_while(|s| s.kind == crate::textdiff::SegKind::Equal)
            .map(|s| s.text.matches('\n').count())
            .sum();
        let section = if new.is_empty() {
            let byte = old_line_starts.get(hunk.old_line - 1 + lead).copied().unwrap_or(0);
            section_at(&old_outline, byte).map(|s| s.title.clone())
        } else {
            let byte = new_line_starts.get(hunk.new_line - 1 + lead).copied().unwrap_or(0);
            section_at(&new_outline, byte).map(|s| s.title.clone())
        };
        match sections.last_mut() {
            Some(s) if s.section == section => s.hunks.push(hunk),
            _ => sections.push(SectionChange { section, hunks: vec![hunk] }),
        }
    }
    Some(DocDiff { path: path.into(), status: status.into(), words_added, words_removed, sections })
}

/// Plain-text rendering of diffs for terminals and agents.
pub fn render_diffs(diffs: &[DocDiff]) -> String {
    if diffs.is_empty() {
        return "No changes.\n".into();
    }
    let mut out = String::new();
    for d in diffs {
        out.push_str(&format!("=== {} ({}, +{} / -{} words)\n", d.path, d.status, d.words_added, d.words_removed));
        for s in &d.sections {
            out.push_str(&format!("--- § {}\n", s.section.as_deref().unwrap_or("(preamble)")));
            out.push_str(&crate::textdiff::render_word_diff(&s.hunks));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_refs() {
        assert_eq!(VersionRef::parse("HEAD"), VersionRef::Now);
        assert_eq!(VersionRef::parse("#42"), VersionRef::Update(42));
        assert_eq!(VersionRef::parse("submitted-v1"), VersionRef::Checkpoint("submitted-v1".into()));
    }

    #[test]
    fn diff_attributes_sections() {
        let old = "\\section{Intro}\nWe use a small model.\n\\section{Method}\nSGD.\n";
        let new = "\\section{Intro}\nWe use a small model.\n\\section{Method}\nAdam with warmup.\n";
        let d = diff_doc("main.tex", Some(old), Some(new)).unwrap();
        assert_eq!(d.sections.len(), 1);
        assert_eq!(d.sections[0].section.as_deref(), Some("Method"));
        assert!(render_diffs(&[d]).contains("{+Adam with warmup.+}"));
    }
}
