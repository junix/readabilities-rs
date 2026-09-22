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
            completed_attempts: Vec::new(),
        }
    }

    pub(crate) fn with_retry(mut self, retry: RetryAdvice) -> Self {
        self.retry = retry;
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
