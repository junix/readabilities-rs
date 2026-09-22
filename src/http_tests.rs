use super::*;

#[test]
fn embedded_private_ipv4_addresses_are_non_public() {
    for raw in [
        "::ffff:127.0.0.1",
        "::ffff:169.254.169.254",
        "::ffff:10.0.0.1",
        "::ffff:192.168.1.1",
        "64:ff9b::7f00:1",
    ] {
        let ip = raw.parse::<IpAddr>().unwrap();
        assert!(is_non_public(ip), "{raw} must be denied");
    }
}

#[test]
fn embedded_public_ipv4_and_public_ipv6_remain_public() {
    for raw in ["::ffff:8.8.8.8", "64:ff9b::808:808", "2606:4700:4700::1111"] {
        let ip = raw.parse::<IpAddr>().unwrap();
        assert!(!is_non_public(ip), "{raw} must remain permitted");
    }
}

#[tokio::test]
async fn network_validation_denies_embedded_private_ipv4_literals() {
    for raw in [
        "http://[::ffff:127.0.0.1]/",
        "http://[::ffff:169.254.169.254]/",
        "http://[64:ff9b::7f00:1]/",
    ] {
        let url = Url::parse(raw).unwrap();
        let error = validate_network_target(&url, false).await.unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidInput, "{raw}");
        assert_eq!(error.stage, Stage::Validate, "{raw}");
        assert!(
            error.message.contains("denied by default"),
            "{raw}: {}",
            error.message
        );
    }
}

#[tokio::test]
async fn network_validation_keeps_public_ipv6_literals_out_of_dns() {
    let expected = "2606:4700:4700::1111".parse::<IpAddr>().unwrap();
    let url = Url::parse("http://[2606:4700:4700::1111]/").unwrap();
    let addresses = validate_network_target(&url, false).await.unwrap();
    assert_eq!(addresses, [SocketAddr::new(expected, 80)]);
}

#[test]
fn cross_origin_redirects_are_denied_by_default() {
    let start = Url::parse("https://example.test/a").unwrap();
    let target = Url::parse("https://other.test/b").unwrap();
    let error = validate_redirect(&target, &origin(&start), false).unwrap_err();
    assert_eq!(error.kind, ErrorKind::OriginHttp);
    assert_eq!(error.message, "cross-origin redirect denied by policy");
    assert_eq!(error.retry, RetryAdvice::ChooseAnotherBackend);
}

#[test]
fn same_origin_relative_redirect_is_allowed() {
    let start = Url::parse("https://example.test/a").unwrap();
    let target = start.join("/b").unwrap();
    assert!(validate_redirect(&target, &origin(&start), false).is_ok());
}
