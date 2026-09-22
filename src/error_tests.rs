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
