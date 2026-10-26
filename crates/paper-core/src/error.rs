use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no paper project found at or above {0} (run `paper init`)")]
    NoProject(PathBuf),
    #[error("document not found: {0}")]
    NoDocument(String),
    #[error("section not found: {0}")]
    NoSection(String),
    #[error("text not found in {path}: {snippet:?}")]
    TextNotFound { path: String, snippet: String },
    #[error("text occurs {count} times in {path}; add surrounding context or pass `occurrence` (1-based)")]
    AmbiguousText { path: String, count: usize },
    #[error("{0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("manifest error: {0}")]
    Manifest(String),
    #[error("crdt error: {0}")]
    Crdt(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
