//! A single paper source file as a Yjs-compatible CRDT text.
//!
//! Each insert carries an `a` (author id) formatting attribute, so per-character
//! authorship travels with the text through merges — that is what blame reads.
//! The JS editor writes the same attribute, and offsets are UTF-16 on both sides.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use yrs::types::text::YChange;
use yrs::updates::decoder::Decode;
use yrs::{
    Any, Doc, GetString, OffsetKind, Options, Out, ReadTxn, StateVector, Text, TextRef, Transact,
    Update,
};

use crate::textdiff::{compute_edits, utf16_offset};
use crate::{Error, Result};

pub const TEXT_NAME: &str = "content";
pub const AUTHOR_ATTR: &str = "a";

pub struct PaperDoc {
    doc: Doc,
    text: TextRef,
}

impl Default for PaperDoc {
    fn default() -> Self {
        Self::new()
    }
}

/// A run of text written by one author.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BlameRun {
    pub author: Option<String>,
    pub text: String,
}

impl PaperDoc {
    pub fn new() -> Self {
        let doc = Doc::with_options(Options {
            offset_kind: OffsetKind::Utf16,
            ..Options::default()
        });
        let text = doc.get_or_insert_text(TEXT_NAME);
        PaperDoc { doc, text }
    }

    pub fn from_updates<I, B>(updates: I) -> Result<Self>
    where
        I: IntoIterator<Item = B>,
        B: AsRef<[u8]>,
    {
        let d = Self::new();
        {
            let mut txn = d.doc.transact_mut();
            for u in updates {
                let update =
                    Update::decode_v1(u.as_ref()).map_err(|e| Error::Crdt(e.to_string()))?;
                txn.apply_update(update)
                    .map_err(|e| Error::Crdt(e.to_string()))?;
            }
        }
        Ok(d)
    }

    pub fn text(&self) -> String {
        self.text.get_string(&self.doc.transact())
    }

    pub fn len_utf16(&self) -> u32 {
        self.text.len(&self.doc.transact())
    }

    pub fn apply_update(&self, update: &[u8]) -> Result<()> {
        let update = Update::decode_v1(update).map_err(|e| Error::Crdt(e.to_string()))?;
        self.doc
            .transact_mut()
            .apply_update(update)
            .map_err(|e| Error::Crdt(e.to_string()))
    }

    pub fn state_vector(&self) -> Vec<u8> {
        use yrs::updates::encoder::Encode;
        self.doc.transact().state_vector().encode_v1()
    }

    /// The whole document as a single update.
    pub fn encode_state(&self) -> Vec<u8> {
        self.doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default())
    }

    /// Everything the holder of `state_vector` is missing.
    pub fn encode_diff(&self, state_vector: &[u8]) -> Result<Vec<u8>> {
        let sv = StateVector::decode_v1(state_vector).map_err(|e| Error::Crdt(e.to_string()))?;
        Ok(self.doc.transact().encode_state_as_update_v1(&sv))
    }

    fn attrs(author: &str) -> yrs::types::Attrs {
        let mut a = HashMap::new();
        a.insert(Arc::<str>::from(AUTHOR_ATTR), Any::from(author.to_string()));
        a
    }

    /// Apply edits expressed as UTF-16 `(start, end, insert)` ranges of the
    /// current text, sorted ascending and non-overlapping. Returns the
    /// resulting update, or `None` if nothing changed.
    pub fn apply_edits(&self, author: &str, edits: &[(u32, u32, String)]) -> Option<Vec<u8>> {
        if edits.iter().all(|(s, e, ins)| s == e && ins.is_empty()) {
            return None;
        }
        let before = self.doc.transact().state_vector();
        {
            let mut txn = self.doc.transact_mut();
            // Apply back to front so earlier offsets stay valid.
            for (start, end, insert) in edits.iter().rev() {
                if end > start {
                    self.text.remove_range(&mut txn, *start, end - start);
                }
                if !insert.is_empty() {
                    self.text
                        .insert_with_attributes(&mut txn, *start, insert, Self::attrs(author));
                }
            }
        }
        Some(self.doc.transact().encode_state_as_update_v1(&before))
    }

    /// Replace a UTF-16 range.
    pub fn splice(&self, author: &str, start: u32, end: u32, insert: &str) -> Option<Vec<u8>> {
        self.apply_edits(author, &[(start, end, insert.to_string())])
    }

    /// Make the text equal to `new_text` with a minimal set of edits, so
    /// untouched passages keep their original authors.
    pub fn set_text(&self, author: &str, new_text: &str) -> Option<Vec<u8>> {
        let old = self.text();
        let edits: Vec<_> = compute_edits(&old, new_text)
            .into_iter()
            .map(|e| {
                (
                    utf16_offset(&old, e.start),
                    utf16_offset(&old, e.end),
                    e.insert,
                )
            })
            .collect();
        self.apply_edits(author, &edits)
    }

    /// Text as runs of contiguous authorship.
    pub fn blame_runs(&self) -> Vec<BlameRun> {
        let txn = self.doc.transact();
        let diffs = self.text.diff(&txn, YChange::identity);
        let mut runs: Vec<BlameRun> = vec![];
        for d in diffs {
            let Out::Any(Any::String(s)) = d.insert else {
                continue;
            };
            let author = d
                .attributes
                .as_ref()
                .and_then(|a| a.get(AUTHOR_ATTR))
                .and_then(|v| match v {
                    Any::String(s) => Some(s.to_string()),
                    _ => None,
                });
            match runs.last_mut() {
                Some(r) if r.author == author => r.text.push_str(&s),
                _ => runs.push(BlameRun {
                    author,
                    text: s.to_string(),
                }),
            }
        }
        runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_edits_merge() {
        let base = PaperDoc::new();
        let u0 = base
            .set_text("human:ada", "The results are good.\n")
            .unwrap();

        let a = PaperDoc::from_updates([&u0]).unwrap();
        let b = PaperDoc::from_updates([&u0]).unwrap();
        let ua = a
            .set_text("human:ada", "The results are very good.\n")
            .unwrap();
        let ub = b
            .set_text("agent:claude", "The results are good.\nWe discuss why.\n")
            .unwrap();
        a.apply_update(&ub).unwrap();
        b.apply_update(&ua).unwrap();
        assert_eq!(a.text(), b.text());
        assert_eq!(a.text(), "The results are very good.\nWe discuss why.\n");
    }

    #[test]
    fn blame_tracks_authors() {
        let d = PaperDoc::new();
        d.set_text("human:ada", "Hello world").unwrap();
        d.set_text("agent:claude", "Hello brave new world").unwrap();
        let runs = d.blame_runs();
        let agent: String = runs
            .iter()
            .filter(|r| r.author.as_deref() == Some("agent:claude"))
            .map(|r| r.text.as_str())
            .collect();
        assert_eq!(agent, "brave new ");
        assert_eq!(
            runs.iter().map(|r| r.text.as_str()).collect::<String>(),
            "Hello brave new world"
        );
    }

    #[test]
    fn utf16_splice() {
        let d = PaperDoc::new();
        d.set_text("x", "a😀b").unwrap();
        d.splice("x", 3, 4, "c").unwrap();
        assert_eq!(d.text(), "a😀c");
    }

    #[test]
    fn noop_produces_no_update() {
        let d = PaperDoc::new();
        d.set_text("x", "same").unwrap();
        assert!(d.set_text("x", "same").is_none());
    }
}
