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

fn headers_with_retry_after(value: &str) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::RETRY_AFTER,
        reqwest::header::HeaderValue::from_str(value).unwrap(),
    );
    headers
}

#[test]
fn map_status_captures_retry_after_only_for_open_retry_advice() {
    // A delta-seconds value on a rate-limited status lands on the error.
    let error = map_status(
        StatusCode::TOO_MANY_REQUESTS,
        &headers_with_retry_after("120"),
    )
    .unwrap_err();
    assert_eq!(error.retry, RetryAdvice::RetryAfter);
    assert_eq!(error.retry_after_secs, Some(120));

    // Terminal advice ignores the header entirely.
    let error = map_status(StatusCode::NOT_FOUND, &headers_with_retry_after("10")).unwrap_err();
    assert_eq!(error.retry, RetryAdvice::Never);
    assert_eq!(error.retry_after_secs, None);

    // Malformed advice leaves the field unset without changing the mapping.
    let error = map_status(
        StatusCode::TOO_MANY_REQUESTS,
        &headers_with_retry_after("soon"),
    )
    .unwrap_err();
    assert_eq!(error.retry, RetryAdvice::RetryAfter);
    assert_eq!(error.retry_after_secs, None);
}

#[test]
fn retry_after_delta_seconds_and_whitespace_are_parsed() {
    let now = SystemTime::UNIX_EPOCH;
    assert_eq!(parse_retry_after("0", now), ParsedRetryAfter::Delay(0));
    assert_eq!(parse_retry_after("120", now), ParsedRetryAfter::Delay(120));
    assert_eq!(parse_retry_after(" 42 ", now), ParsedRetryAfter::Delay(42));
}

#[test]
fn retry_after_malformed_values_are_reported_as_malformed() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(784_111_777);
    for raw in [
        "",
        " ",
        "soon",
        "-5",
        "1.5",
        "120s",
        "6 Nov 1994",
        "Sun, 06 Nov 1994 08:49:37",     // missing zone
        "Sun, 06 Nov 1994 08:49:37 PDT", // non-GMT zone
        "Sun, Nov 1994 08:49:37 GMT",    // missing day of month
        "Sun, 06 Nov 1994 08:49 GMT",    // incomplete clock
    ] {
        assert_eq!(
            parse_retry_after(raw, now),
            ParsedRetryAfter::Malformed,
            "{raw:?}"
        );
    }
}

#[test]
fn retry_after_http_dates_use_the_injected_clock() {
    // Reference clock pinned to the RFC example date: 1994-11-06T08:49:37Z.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(784_111_777);

    // 2038-01-19T03:14:07Z, 1_363_371_870 seconds ahead. The day name is
    // redundant and ignored by the parser.
    let future = parse_retry_after("Tue, 19 Jan 2038 03:14:07 GMT", now);
    assert_eq!(future, ParsedRetryAfter::Delay(1_363_371_870));

    // The reference instant itself has no wait left, so it is not past.
    let exact = parse_retry_after("Sun, 06 Nov 1994 08:49:37 GMT", now);
    assert_eq!(exact, ParsedRetryAfter::Delay(0));

    // One second behind the clock: the advised wait has already elapsed.
    let past = parse_retry_after("Sun, 06 Nov 1994 08:49:36 GMT", now);
    assert_eq!(past, ParsedRetryAfter::Past);
}

#[test]
fn http_date_parser_matches_reference_epoch_values() {
    for (raw, epoch_secs) in [
        ("Thu, 01 Jan 1970 00:00:00 GMT", 0_u64),
        ("Sun, 06 Nov 1994 08:49:37 GMT", 784_111_777),
        ("Sat, 01 Jan 2000 00:00:00 GMT", 946_684_800),
        ("Tue, 19 Jan 2038 03:14:07 GMT", 2_147_483_647),
    ] {
        let parsed = parse_http_date(raw).unwrap();
        assert_eq!(
            parsed
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            epoch_secs,
            "{raw}"
        );
    }
    assert_eq!(parse_http_date("Sun, 32 Nov 1994 08:49:37 GMT"), None);
    assert_eq!(parse_http_date("Sun, 06 Foo 1994 08:49:37 GMT"), None);
    assert_eq!(parse_http_date("Sun, 06 Nov 10000 08:49:37 GMT"), None);
}
