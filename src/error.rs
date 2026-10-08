use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{AttemptRecord, Backend, Stage};

pub type Result<T> = std::result::Result<T, ReadError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    InvalidInput,
    Unsupported,
    OriginHttp,
    Decode,
    Parse,
    NoContent,
    Authentication,
    RateLimit,
    Timeout,
    BudgetExceeded,
    DepthExceeded,
    Cancelled,
    Render,
    InternalInvariant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryAdvice {
    Never,
    RetrySameBackend,
    RetryAfter,
    ChooseAnotherBackend,
    IncreaseBudget,
}

#[derive(Debug, Clone, Serialize, Deserialize, Error)]
#[error("{kind:?} during {stage:?}: {message}")]
pub struct ReadError {
    pub kind: ErrorKind,
    pub stage: Stage,
    pub backend: Backend,
    pub message: String,
    pub retry: RetryAdvice,
    /// Server-advised wait in seconds parsed from a `Retry-After` response
    /// header, captured only when the origin sent a valid delta-seconds or
    /// HTTP-date value on a status whose retry advice leaves the retry
    /// decision open. Absent and malformed headers leave it `None`; a date
    /// already in the past is captured as zero. Optional so errors serialized
    /// by older versions keep deserializing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completed_attempts: Vec<AttemptRecord>,
}

impl ReadError {
    pub(crate) fn new(
        kind: ErrorKind,
        stage: Stage,
        backend: Backend,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            stage,
            backend,
            message: redact(&message.into()),
            retry: RetryAdvice::Never,
            retry_after_secs: None,
            completed_attempts: Vec::new(),
        }
    }

    pub(crate) fn with_retry(mut self, retry: RetryAdvice) -> Self {
        self.retry = retry;
        self
    }

    pub(crate) fn with_retry_after_secs(mut self, retry_after_secs: u64) -> Self {
        self.retry_after_secs = Some(retry_after_secs);
        self
    }

    pub(crate) fn with_attempts(mut self, attempts: Vec<AttemptRecord>) -> Self {
        self.completed_attempts = attempts;
        self
    }
}

fn redact(message: &str) -> String {
    const SECRET_MARKERS: [&str; 5] = ["authorization", "api_key", "api-key", "token", "cookie"];
    let lowercase = message.to_ascii_lowercase();
    if SECRET_MARKERS
        .iter()
        .any(|marker| lowercase.contains(marker))
    {
        "backend returned a redacted error containing credential-like data".to_string()
    } else {
        message.chars().take(500).collect()
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
