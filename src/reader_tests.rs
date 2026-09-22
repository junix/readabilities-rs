use super::*;

#[tokio::test]
async fn execute_preserves_no_content_as_failure() {
    let execution = Reader::new().execute(ReadRequest::html("   ", None)).await;
    let ExecutionOutcome::Failure(error) = &execution.outcome else {
        panic!("blank input must fail validation");
    };
    assert_eq!(error.kind, ErrorKind::InvalidInput);
    assert_eq!(error.stage, Stage::Validate);
    assert_eq!(error.message, "HTML input is empty");
    // Validation refused the input before any work: no attempts, no
    // stages, and an untouched cost ledger.
    assert!(execution.attempts.is_empty());
    assert!(execution.stages.is_empty());
    assert_eq!(execution.cost.origin_requests, 0);
    assert_eq!(execution.cost.downloaded_bytes, 0);
}

#[tokio::test]
async fn rejects_embedded_url_credentials_before_network() {
    let url = Url::parse("https://user:secret@example.test/article").unwrap();
    let execution = Reader::new().execute(ReadRequest::url(url)).await;
    let ExecutionOutcome::Failure(error) = &execution.outcome else {
        panic!("credentialed URLs must fail validation");
    };
    assert_eq!(error.kind, ErrorKind::InvalidInput);
    assert_eq!(error.stage, Stage::Validate);
    assert_eq!(error.message, "embedded URL credentials are forbidden");
    // The refusal happens before DNS or sockets: the ledger stays zeroed.
    assert_eq!(execution.cost.origin_requests, 0);
    assert_eq!(execution.cost.downloaded_bytes, 0);
}

#[test]
fn rejects_invalid_library_site_config_before_extraction() {
    let config = SiteConfig {
        id: "invalid".to_string(),
        hosts: vec!["example.test".to_string()],
        path_prefixes: Vec::new(),
        content_selector: Some("main[".to_string()),
        remove_selectors: Vec::new(),
    };

    let error = Reader::with_options_and_site_configs(ExtractionOptions::default(), vec![config])
        .expect_err("an invalid selector must reject the config");
    assert!(
        matches!(
            &error,
            SiteConfigError::Invalid(message)
                if message.starts_with("site invalid selector \"main[\"")
        ),
        "unexpected error: {error}"
    );
}
