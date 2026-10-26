//! Text differencing used for two jobs:
//! 1. turning "the file now looks like this" into minimal CRDT edits, so that
//!    attribution and concurrent edits survive whole-file rewrites;
//! 2. human-readable, word-level diffs for paper history.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use similar::{Algorithm, ChangeTag, DiffOp, TextDiff};

/// Replace `old[start..end]` (byte offsets) with `insert`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub insert: String,
}

/// Minimal-ish edits turning `old` into `new`, sorted and non-overlapping.
///
/// Common prefix/suffix are trimmed first (the usual case is one local edit),
/// then a line diff finds changed regions and a char diff refines each region.
pub fn compute_edits(old: &str, new: &str) -> Vec<Edit> {
    if old == new {
        return vec![];
    }
    let prefix = common_prefix(old, new);
    let suffix = common_suffix(&old[prefix..], &new[prefix..]);
    let old_mid = &old[prefix..old.len() - suffix];
    let new_mid = &new[prefix..new.len() - suffix];

    let deadline = Instant::now() + Duration::from_millis(250);
    let lines = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .deadline(deadline)
        .diff_lines(old_mid, new_mid);
    let old_starts = line_starts(old_mid);
    let new_starts = line_starts(new_mid);

    let mut edits = vec![];
    // Collapse runs of non-equal ops into (old_range, new_range) regions.
    let mut region: Option<(usize, usize, usize, usize)> = None;
    let flush = |r: Option<(usize, usize, usize, usize)>, edits: &mut Vec<Edit>| {
        if let Some((o0, o1, n0, n1)) = r {
            let ob = (old_starts[o0], old_starts[o1]);
            let nb = (new_starts[n0], new_starts[n1]);
            refine(&old_mid[ob.0..ob.1], &new_mid[nb.0..nb.1], prefix + ob.0, deadline, edits);
        }
    };
    for op in lines.ops() {
        match op {
            DiffOp::Equal { .. } => flush(region.take(), &mut edits),
            _ => {
                let (o, n) = (op.old_range(), op.new_range());
                region = Some(match region {
                    Some((o0, _, n0, _)) => (o0, o.end, n0, n.end),
                    None => (o.start, o.end, n.start, n.end),
                });
            }
        }
    }
    flush(region.take(), &mut edits);
    edits
}

/// Char-level diff inside one changed region; `base` is its byte offset in the full text.
fn refine(old: &str, new: &str, base: usize, deadline: Instant, out: &mut Vec<Edit>) {
    if old.is_empty() || new.is_empty() || old.len() + new.len() > 20_000 {
        out.push(Edit { start: base, end: base + old.len(), insert: new.to_string() });
        return;
    }
    let diff = TextDiff::configure().deadline(deadline).diff_chars(old, new);
    let old_chars: Vec<usize> = old.char_indices().map(|(i, _)| i).chain([old.len()]).collect();
    let new_chars: Vec<usize> = new.char_indices().map(|(i, _)| i).chain([new.len()]).collect();
    let mut pending: Option<Edit> = None;
    for op in diff.ops() {
        if let DiffOp::Equal { .. } = op {
            out.extend(pending.take());
            continue;
        }
        let (o, n) = (op.old_range(), op.new_range());
        let (os, oe) = (old_chars[o.start], old_chars[o.end]);
        let ins = &new[new_chars[n.start]..new_chars[n.end]];
        match &mut pending {
            Some(e) if e.end == base + os => {
                e.end = base + oe;
                e.insert.push_str(ins);
            }
            _ => {
                out.extend(pending.take());
                pending = Some(Edit { start: base + os, end: base + oe, insert: ins.to_string() });
            }
        }
    }
    out.extend(pending);
}

fn common_prefix(a: &str, b: &str) -> usize {
    let mut n = 0;
    for (ca, cb) in a.chars().zip(b.chars()) {
        if ca != cb {
            break;
        }
        n += ca.len_utf8();
    }
    n
}

fn common_suffix(a: &str, b: &str) -> usize {
    let mut n = 0;
    for (ca, cb) in a.chars().rev().zip(b.chars().rev()) {
        if ca != cb {
            break;
        }
        n += ca.len_utf8();
    }
    n
}

/// Byte offset of the start of each line, plus a final entry for the end.
fn line_starts(s: &str) -> Vec<usize> {
    let mut v = vec![0];
    let mut acc = 0;
    for line in s.split_inclusive('\n') {
        acc += line.len();
        v.push(acc);
    }
    if v.len() == 1 {
        v.push(s.len());
    }
    v
}

/// Number of UTF-16 code units in `s[..byte]`. CRDT offsets use UTF-16 so they
/// agree with the JavaScript editor.
pub fn utf16_offset(s: &str, byte: usize) -> u32 {
    s[..byte].encode_utf16().count() as u32
}

/// Byte offset of a UTF-16 offset (clamped to the text).
pub fn byte_offset(s: &str, utf16: u32) -> usize {
    let mut units = 0u32;
    for (i, c) in s.char_indices() {
        if units >= utf16 {
            return i;
        }
        units += c.len_utf16() as u32;
    }
    s.len()
}

/// 1-based line number of a byte offset.
pub fn line_of(s: &str, byte: usize) -> usize {
    s[..byte.min(s.len())].matches('\n').count() + 1
}

/// Count of words added and removed between two texts.
pub fn word_stats(old: &str, new: &str) -> (usize, usize) {
    let diff = TextDiff::configure()
        .deadline(Instant::now() + Duration::from_millis(250))
        .diff_words(old, new);
    let (mut added, mut removed) = (0, 0);
    for change in diff.iter_all_changes() {
        if change.value().trim().is_empty() {
            continue;
        }
        match change.tag() {
            ChangeTag::Insert => added += 1,
            ChangeTag::Delete => removed += 1,
            ChangeTag::Equal => {}
        }
    }
    (added, removed)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SegKind {
    Equal,
    Insert,
    Delete,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Segment {
    pub kind: SegKind,
    pub text: String,
}

/// One changed passage with a little surrounding context.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hunk {
    pub old_line: usize,
    pub new_line: usize,
    pub segments: Vec<Segment>,
}

/// Word-level hunks between two texts, `context` lines around each change.
pub fn word_hunks(old: &str, new: &str, context: usize) -> Vec<Hunk> {
    let deadline = Instant::now() + Duration::from_millis(500);
    let lines = TextDiff::configure().deadline(deadline).diff_lines(old, new);
    let old_starts = line_starts(old);
    let new_starts = line_starts(new);
    let mut hunks = vec![];
    for group in lines.grouped_ops(context) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else { continue };
        let o = first.old_range().start..last.old_range().end;
        let n = first.new_range().start..last.new_range().end;
        let old_seg = &old[old_starts[o.start.min(old_starts.len() - 1)]..old_starts[o.end.min(old_starts.len() - 1)]];
        let new_seg = &new[new_starts[n.start.min(new_starts.len() - 1)]..new_starts[n.end.min(new_starts.len() - 1)]];
        let words = TextDiff::configure().deadline(deadline).diff_words(old_seg, new_seg);
        let mut segments: Vec<Segment> = vec![];
        for change in words.iter_all_changes() {
            let kind = match change.tag() {
                ChangeTag::Equal => SegKind::Equal,
                ChangeTag::Insert => SegKind::Insert,
                ChangeTag::Delete => SegKind::Delete,
            };
            match segments.last_mut() {
                Some(s) if s.kind == kind => s.text.push_str(change.value()),
                _ => segments.push(Segment { kind, text: change.value().to_string() }),
            }
        }
        hunks.push(Hunk { old_line: o.start + 1, new_line: n.start + 1, segments });
    }
    hunks
}

/// Render hunks like `git diff --word-diff`: `[-removed-]{+added+}`.
pub fn render_word_diff(hunks: &[Hunk]) -> String {
    let mut out = String::new();
    for h in hunks {
        out.push_str(&format!("@@ line {} → {} @@\n", h.old_line, h.new_line));
        for s in &h.segments {
            match s.kind {
                SegKind::Equal => out.push_str(&s.text),
                SegKind::Insert => out.push_str(&format!("{{+{}+}}", s.text)),
                SegKind::Delete => out.push_str(&format!("[-{}-]", s.text)),
            }
        }
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(old: &str, edits: &[Edit]) -> String {
        let mut s = old.to_string();
        for e in edits.iter().rev() {
            s.replace_range(e.start..e.end, &e.insert);
        }
        s
    }

    #[test]
    fn edits_roundtrip() {
        let cases = [
            ("", "hello"),
            ("hello", ""),
            ("the cat sat\non the mat\n", "the cat sat\non a mat\nand slept\n"),
            ("α β γ\nδ\n", "α b γ\nδ ε\n"),
            ("a\nb\nc\nd\n", "a\nc\nb\nd\n"),
            ("😀 emoji line\nsecond", "😀 emoji lines\nsecond 😀"),
        ];
        for (old, new) in cases {
            let edits = compute_edits(old, new);
            assert_eq!(apply(old, &edits), new, "{old:?} -> {new:?}: {edits:?}");
        }
    }

    #[test]
    fn local_edit_is_small() {
        let old = "We propose a method.\n".repeat(50);
        let new = old.replacen("a method", "a new method", 1);
        let edits = compute_edits(&old, &new);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].insert, "new ");
    }

    #[test]
    fn utf16_conversions() {
        let s = "a😀b";
        assert_eq!(utf16_offset(s, 5), 3);
        assert_eq!(byte_offset(s, 3), 5);
    }

    #[test]
    fn word_diff_marks_changes() {
        let hunks = word_hunks("We use a small model.\n", "We use a large model.\n", 1);
        let text = render_word_diff(&hunks);
        assert!(text.contains("[-small-]{+large+}"), "{text}");
        assert_eq!(word_stats("a b c", "a x c"), (1, 1));
    }
}
