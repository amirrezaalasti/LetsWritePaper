//! Peer review: review rounds (one reviewer, one version of the paper) holding
//! comments anchored to verbatim quotes, plus author responses.

use serde::{Deserialize, Serialize};

use crate::author::Author;

pub const SEVERITIES: &[&str] = &["major", "minor", "nit", "question"];
pub const CATEGORIES: &[&str] = &[
    "soundness",
    "novelty",
    "clarity",
    "experiments",
    "related-work",
    "reproducibility",
    "writing",
    "formatting",
    "ethics",
];
pub const COMMENT_STATUSES: &[&str] = &["open", "addressed", "disputed", "wontfix"];

/// Built-in reviewer personas; the `peer-review` skill describes each in depth.
pub const PERSONAS: &[(&str, &str)] = &[
    ("methods-skeptic", "Probes assumptions, baselines, statistics and whether the evidence supports each claim."),
    ("clarity-reviewer", "Reads as a non-expert: structure, definitions, figures, and whether the story is followable."),
    ("novelty-reviewer", "Positions the work against prior art and asks what is genuinely new."),
    ("reproducibility-reviewer", "Checks that a skilled reader could re-implement and re-run everything."),
    ("area-chair", "Writes a meta-review weighing the other reviews and the paper's overall contribution."),
];

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Scores {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soundness: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contribution: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overall: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewRound {
    pub id: i64,
    /// Global id, stable across devices.
    pub uid: String,
    pub title: String,
    pub persona: Option<String>,
    pub venue: Option<String>,
    pub reviewer: Author,
    /// Last history update included in the reviewed version.
    pub update_id: i64,
    pub status: String,
    pub summary: Option<String>,
    pub scores: Option<Scores>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewComment {
    pub id: i64,
    pub uid: String,
    pub round_id: i64,
    pub round_uid: Option<String>,
    pub doc: String,
    pub section: Option<String>,
    /// Verbatim text the comment refers to.
    pub quote: String,
    /// UTF-16 offset of the quote when the comment was made; disambiguates repeats.
    pub offset_hint: Option<i64>,
    pub severity: String,
    pub category: String,
    pub body: String,
    pub status: String,
    pub response: Option<String>,
    pub responder: Option<Author>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Where the quote currently is (1-based line); `None` when the quoted text
    /// no longer exists, i.e. it was rewritten since the review.
    #[serde(default)]
    pub current_line: Option<usize>,
}

pub fn validate_severity(s: &str) -> crate::Result<()> {
    check(s, SEVERITIES, "severity")
}

pub fn validate_category(s: &str) -> crate::Result<()> {
    check(s, CATEGORIES, "category")
}

pub fn validate_status(s: &str) -> crate::Result<()> {
    check(s, COMMENT_STATUSES, "status")
}

fn check(s: &str, allowed: &[&str], what: &str) -> crate::Result<()> {
    if allowed.contains(&s) {
        Ok(())
    } else {
        Err(crate::Error::Invalid(format!(
            "invalid {what} {s:?}; expected one of {allowed:?}"
        )))
    }
}

/// Render rounds and their comments as a Markdown rebuttal / response letter.
pub fn render_response_letter(title: &str, rounds: &[(ReviewRound, Vec<ReviewComment>)]) -> String {
    let mut out = format!("# Response to reviewers — {title}\n\n");
    out.push_str(
        "We thank the reviewers for their feedback. Responses are given point by point below.\n",
    );
    for (round, comments) in rounds {
        let who = round
            .persona
            .clone()
            .unwrap_or_else(|| round.reviewer.name.clone());
        out.push_str(&format!("\n## {} ({})\n", round.title, who));
        if let Some(summary) = &round.summary {
            out.push_str(&format!("\n> {}\n", summary.replace('\n', "\n> ")));
        }
        for c in comments {
            out.push_str(&format!(
                "\n### R{}.{} [{} · {}] — {}\n\n> {}\n\n",
                round.id,
                c.id,
                c.severity,
                c.category,
                c.status,
                c.body.replace('\n', "\n> ")
            ));
            match &c.response {
                Some(r) => out.push_str(&format!("**Response.** {r}\n")),
                None => out.push_str("**Response.** _TODO_\n"),
            }
        }
    }
    out
}
