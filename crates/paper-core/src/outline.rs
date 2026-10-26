//! Section structure of LaTeX, Typst and Markdown sources.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Section {
    pub level: u8,
    pub title: String,
    /// `\label{...}` / `<label>` attached to the heading, if any.
    pub label: Option<String>,
    /// 1-based line of the heading.
    pub line: usize,
    /// 1-based last line of the section (inclusive), including subsections.
    pub end_line: usize,
    /// Byte range of the whole section, heading included.
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Syntax {
    Latex,
    Typst,
    Markdown,
}

impl Syntax {
    pub fn from_path(path: &str) -> Option<Self> {
        match path.rsplit('.').next()? {
            "tex" => Some(Syntax::Latex),
            "typ" => Some(Syntax::Typst),
            "md" => Some(Syntax::Markdown),
            _ => None,
        }
    }
}

struct Heading {
    level: u8,
    title: String,
    label: Option<String>,
    line: usize,
    start: usize,
    /// Explicit end (for environments such as `abstract`), else computed.
    end: Option<usize>,
}

pub fn outline(path: &str, text: &str) -> Vec<Section> {
    let Some(syntax) = Syntax::from_path(path) else {
        return vec![];
    };
    let heads = match syntax {
        Syntax::Latex => latex_headings(text),
        Syntax::Typst => typst_headings(text),
        Syntax::Markdown => markdown_headings(text),
    };
    let mut sections = Vec::with_capacity(heads.len());
    for (i, h) in heads.iter().enumerate() {
        let end = h.end.unwrap_or_else(|| {
            heads[i + 1..]
                .iter()
                .find(|n| n.level <= h.level && n.end.is_none())
                .map(|n| n.start)
                .unwrap_or(text.len())
        });
        let end_line = crate::textdiff::line_of(text, end.saturating_sub(1).max(h.start));
        sections.push(Section {
            level: h.level,
            title: h.title.clone(),
            label: h.label.clone(),
            line: h.line,
            end_line,
            start: h.start,
            end,
        });
    }
    sections
}

/// Find a section by title or label: exact (case-insensitive), then prefix, then substring.
pub fn find_section<'a>(sections: &'a [Section], query: &str) -> Option<&'a Section> {
    let q = query.trim().to_lowercase();
    let q = q.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ');
    sections
        .iter()
        .find(|s| {
            s.title.to_lowercase() == q
                || s.label.as_deref().map(str::to_lowercase).as_deref() == Some(q)
        })
        .or_else(|| {
            sections
                .iter()
                .find(|s| s.title.to_lowercase().starts_with(q))
        })
        .or_else(|| sections.iter().find(|s| s.title.to_lowercase().contains(q)))
}

/// The innermost section containing a byte offset.
pub fn section_at(sections: &[Section], byte: usize) -> Option<&Section> {
    sections
        .iter()
        .filter(|s| s.start <= byte && byte < s.end.max(s.start + 1))
        .max_by_key(|s| s.level)
}

fn line_iter(text: &str) -> impl Iterator<Item = (usize, usize, &str)> {
    let mut offset = 0;
    text.split_inclusive('\n')
        .enumerate()
        .map(move |(i, line)| {
            let start = offset;
            offset += line.len();
            (i + 1, start, line.trim_end_matches(['\n', '\r']))
        })
}

const LATEX_LEVELS: &[(&str, u8)] = &[
    ("part", 0),
    ("chapter", 0),
    ("section", 1),
    ("subsection", 2),
    ("subsubsection", 3),
    ("paragraph", 4),
];

fn latex_headings(text: &str) -> Vec<Heading> {
    let mut heads = vec![];
    let mut abstract_start: Option<(usize, usize)> = None;
    for (line_no, start, line) in line_iter(text) {
        let trimmed = line.trim_start();
        if trimmed.starts_with('%') {
            continue;
        }
        if trimmed.starts_with("\\begin{abstract}") {
            abstract_start = Some((line_no, start));
            continue;
        }
        if trimmed.starts_with("\\end{abstract}") {
            if let Some((l, s)) = abstract_start.take() {
                heads.push(Heading {
                    level: 1,
                    title: "Abstract".into(),
                    label: None,
                    line: l,
                    start: s,
                    end: Some(start + line.len()),
                });
            }
            continue;
        }
        let Some(rest) = trimmed.strip_prefix('\\') else {
            continue;
        };
        for (cmd, level) in LATEX_LEVELS {
            let Some(after) = rest.strip_prefix(cmd) else {
                continue;
            };
            let after = after.strip_prefix('*').unwrap_or(after).trim_start();
            // Optional short title: \section[short]{Long}
            let after = if after.starts_with('[') {
                match after.find(']') {
                    Some(i) => after[i + 1..].trim_start(),
                    None => continue,
                }
            } else {
                after
            };
            let Some(title) = braced(after) else { continue };
            let label = after
                .find("\\label{")
                .and_then(|i| braced(&after[i + 6..]))
                .or_else(|| next_line_label(text, start + line.len()));
            heads.push(Heading {
                level: *level,
                title: clean_title(&title),
                label,
                line: line_no,
                start,
                end: None,
            });
            break;
        }
    }
    heads.sort_by_key(|h| h.start);
    heads
}

/// A `\label{..}` on the line right after a heading.
fn next_line_label(text: &str, from: usize) -> Option<String> {
    let next = text
        .get(from..)?
        .trim_start_matches(['\r', '\n'])
        .lines()
        .next()?
        .trim();
    next.strip_prefix("\\label").and_then(braced)
}

/// Contents of a leading `{...}`, honouring nested braces.
fn braced(s: &str) -> Option<String> {
    let s = s.strip_prefix('{')?;
    let mut depth = 1;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[..i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn clean_title(t: &str) -> String {
    t.replace(['{', '}', '~'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn typst_headings(text: &str) -> Vec<Heading> {
    let mut heads = vec![];
    let mut in_raw = false;
    for (line_no, start, line) in line_iter(text) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_raw = !in_raw;
            continue;
        }
        if in_raw || trimmed.starts_with("//") {
            continue;
        }
        let level = trimmed.chars().take_while(|&c| c == '=').count();
        if level > 0 && trimmed[level..].starts_with(' ') {
            let mut title = trimmed[level..].trim().to_string();
            let mut label = None;
            if let Some(i) = title.rfind('<') {
                if title.ends_with('>') {
                    label = Some(title[i + 1..title.len() - 1].to_string());
                    title = title[..i].trim().to_string();
                }
            }
            heads.push(Heading {
                level: level as u8,
                title,
                label,
                line: line_no,
                start,
                end: None,
            });
        } else if let Some(rest) = trimmed.strip_prefix("#heading(") {
            // `#heading(level: 1, numbering: none)[Abstract]`
            let level = rest
                .split("level:")
                .nth(1)
                .and_then(|s| s.trim_start().chars().next())
                .and_then(|c| c.to_digit(10))
                .unwrap_or(1) as u8;
            if let Some(i) = rest.find(")[") {
                let body = &rest[i + 2..];
                if let Some(j) = body.find(']') {
                    heads.push(Heading {
                        level,
                        title: body[..j].trim().to_string(),
                        label: None,
                        line: line_no,
                        start,
                        end: None,
                    });
                }
            }
        }
    }
    heads
}

fn markdown_headings(text: &str) -> Vec<Heading> {
    let mut heads = vec![];
    let mut in_code = false;
    for (line_no, start, line) in line_iter(text) {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        let level = line.chars().take_while(|&c| c == '#').count();
        if !in_code && (1..=6).contains(&level) && line[level..].starts_with(' ') {
            heads.push(Heading {
                level: level as u8,
                title: line[level..].trim().to_string(),
                label: None,
                line: line_no,
                start,
                end: None,
            });
        }
    }
    heads
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEX: &str = r"\documentclass{article}
\begin{document}
\begin{abstract}
We study things.
\end{abstract}
\section{Introduction}
\label{sec:intro}
Intro text.
% \section{Commented}
\subsection*{Our {\em Key} Idea}
Idea.
\section[Short]{Related Work}\label{sec:rw}
RW.
\end{document}
";

    #[test]
    fn latex_outline() {
        let s = outline("main.tex", TEX);
        let titles: Vec<_> = s.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Abstract",
                "Introduction",
                "Our \\em Key Idea",
                "Related Work"
            ]
        );
        assert_eq!(s[1].label.as_deref(), Some("sec:intro"));
        assert_eq!(s[3].label.as_deref(), Some("sec:rw"));
        // Introduction spans its subsection and stops at Related Work.
        assert_eq!(s[1].line, 6);
        assert_eq!(s[1].end_line, 11);
        assert!(TEX[s[1].start..s[1].end].contains("Idea."));
        assert_eq!(find_section(&s, "intro").unwrap().title, "Introduction");
        assert_eq!(find_section(&s, "sec:rw").unwrap().title, "Related Work");
        assert_eq!(s[0].end_line, 5);
    }

    #[test]
    fn typst_outline() {
        let src = "#heading(level: 1, numbering: none)[Abstract]\nText\n= Intro <sec:intro>\nA\n== Detail\nB\n```\n= not a heading\n```\n= Method\n";
        let s = outline("main.typ", src);
        let titles: Vec<_> = s.iter().map(|s| (s.level, s.title.as_str())).collect();
        assert_eq!(
            titles,
            [(1, "Abstract"), (1, "Intro"), (2, "Detail"), (1, "Method")]
        );
        assert_eq!(s[1].label.as_deref(), Some("sec:intro"));
        assert_eq!(
            section_at(&s, src.find("B").unwrap()).unwrap().title,
            "Detail"
        );
    }
}
