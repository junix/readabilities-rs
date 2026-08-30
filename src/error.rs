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
mod tests {
    use super::*;

    const REDACTED: &str = "backend returned a redacted error containing credential-like data";

    fn message_for(raw: &str) -> String {
        ReadError::new(ErrorKind::OriginHttp, Stage::Acquire, Backend::Origin, raw).message
    }

    #[test]
    fn credential_like_messages_are_redacted() {
        // The whole message is replaced, not just the secret substring.
        assert_eq!(message_for("Authorization: Bearer secret-token"), REDACTED);
    }

    #[test]
    fn every_secret_marker_triggers_redaction_case_insensitively() {
        for raw in [
            "AUTHORIZATION: Bearer x",
            "header api_key: abc",
            "sent api-key: abc",
            "refresh token expired",
            "Cookie: session=1",
        ] {
            assert_eq!(message_for(raw), REDACTED, "{raw:?}");
        }
    }

    #[test]
    fn ordinary_messages_pass_through_unchanged() {
        assert_eq!(
            message_for("origin returned HTTP 503"),
            "origin returned HTTP 503"
        );
    }

    #[test]
    fn ordinary_messages_are_cut_at_exactly_five_hundred_chars() {
        assert_eq!(message_for(&"y".repeat(600)), "y".repeat(500));
        // At exactly the cap nothing is dropped.
        assert_eq!(message_for(&"y".repeat(500)), "y".repeat(500));
    }
}
