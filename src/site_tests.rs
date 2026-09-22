use super::*;

#[test]
fn host_matching_is_boundary_aware() {
    let config = &builtins()[0];
    assert!(config.matches(&Url::parse("https://medium.com/post").unwrap()));
    assert!(config.matches(&Url::parse("https://team.medium.com/post").unwrap()));
    assert!(!config.matches(&Url::parse("https://notmedium.com/post").unwrap()));
}

#[test]
fn configured_noise_is_removed_before_generic_extraction() {
    let config = &builtins()[0];
    let output = remove_configured_noise(
        r#"<article><p>keep</p><div data-testid="post-preview">remove</div></article>"#,
        config,
    )
    .unwrap();
    assert!(output.contains("keep"));
    assert!(!output.contains("remove"));
}

fn base_config() -> SiteConfig {
    SiteConfig {
        id: "demo".to_string(),
        hosts: vec!["example.test".to_string()],
        path_prefixes: Vec::new(),
        content_selector: None,
        remove_selectors: Vec::new(),
    }
}

#[test]
fn invalid_site_configs_are_rejected_with_specific_messages() {
    for (config, expected) in [
        (
            SiteConfig {
                id: "  ".to_string(),
                ..base_config()
            },
            "site id must not be empty",
        ),
        (
            SiteConfig {
                hosts: Vec::new(),
                ..base_config()
            },
            "site demo must declare at least one host",
        ),
        (
            SiteConfig {
                hosts: vec![String::new()],
                ..base_config()
            },
            "site demo has invalid host \"\"",
        ),
        (
            SiteConfig {
                hosts: vec!["example.test/path".to_string()],
                ..base_config()
            },
            "site demo has invalid host \"example.test/path\"",
        ),
        (
            SiteConfig {
                hosts: vec!["example.test:8080".to_string()],
                ..base_config()
            },
            "site demo has invalid host \"example.test:8080\"",
        ),
        (
            SiteConfig {
                hosts: vec!["https://example.test".to_string()],
                ..base_config()
            },
            "site demo has invalid host \"https://example.test\"",
        ),
        (
            SiteConfig {
                content_selector: Some("main[".to_string()),
                ..base_config()
            },
            "site demo selector \"main[\"",
        ),
        (
            SiteConfig {
                remove_selectors: vec!["div[".to_string()],
                ..base_config()
            },
            "site demo selector \"div[\"",
        ),
    ] {
        let error = config
            .validate()
            .err()
            .unwrap_or_else(|| panic!("config must be rejected: {expected}"));
        let SiteConfigError::Invalid(message) = &error else {
            panic!("expected Invalid, got {error}");
        };
        assert!(message.contains(expected), "{expected} not in {message}");
    }
    // The baseline config stays valid.
    base_config().validate().unwrap();
}

#[test]
fn site_config_documents_accept_single_many_and_wrapped_shapes() {
    for json in [
        r#"{"id":"a","hosts":["a.test"]}"#,
        r#"[{"id":"a","hosts":["a.test"]}]"#,
        r#"{"sites":[{"id":"a","hosts":["a.test"]}]}"#,
    ] {
        let configs = parse_site_configs(json).unwrap();
        assert_eq!(configs.len(), 1, "{json}");
        assert_eq!(configs[0].id, "a", "{json}");
        assert_eq!(configs[0].hosts, ["a.test"], "{json}");
    }
    // Malformed JSON and structurally-valid-but-invalid configs are both
    // rejected before any reader is built.
    assert!(matches!(
        parse_site_configs("not json"),
        Err(SiteConfigError::Json(_))
    ));
    assert!(matches!(
        parse_site_configs(r#"{"id":"a","hosts":[]}"#),
        Err(SiteConfigError::Invalid(_))
    ));
}
