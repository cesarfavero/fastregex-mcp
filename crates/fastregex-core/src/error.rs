use std::path::PathBuf;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, FastRegexError>;

#[derive(Debug, Error)]
pub enum FastRegexError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("glob error: {0}")]
    Glob(String),

    #[error("regex compile error: {0}")]
    RegexCompile(String),

    #[error("index corruption detected: {0}")]
    CorruptIndex(String),

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("operation timed out after {timeout_ms}ms (request_id={request_id:?})")]
    Timeout {
        request_id: Option<String>,
        timeout_ms: u64,
    },

    #[error("utf-8 decode error at {path}: {message}")]
    Utf8 { path: PathBuf, message: String },

    #[error("background rebuild already running")]
    RebuildAlreadyRunning,

    #[error("internal error: {0}")]
    Internal(String),
}
