#![cfg(feature = "http")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use readabilities_rs::{
    ErrorKind, Execution, ExecutionOutcome, ReadError, ReadRequest, Reader, RetryAdvice,
    SnapshotObservations, Stage, UrlPolicy,
};
use url::Url;

fn serve_once(response: String) -> Url {
    serve_bytes(response.into_bytes())
}

fn serve_bytes(response: Vec<u8>) -> Url {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        stream.write_all(&response).unwrap();
    });
    Url::parse(&format!("http://{address}/article")).unwrap()
}

/// Serve one response per accepted connection, in order, recording each
/// request so tests can assert the exact request line and headers.
fn serve_sequence(responses: Vec<Vec<u8>>) -> (Url, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);
    thread::spawn(move || {
        for response in responses {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = Vec::new();
            let mut chunk = [0_u8; 4096];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                request.extend_from_slice(&chunk[..read]);
            }
            recorded
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&request).into_owned());
            stream.write_all(&response).unwrap();
        }
    });
    (
        Url::parse(&format!("http://{address}/article")).unwrap(),
        requests,
    )
}

fn html_response(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn redirect_response(location: &str) -> Vec<u8> {
    format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()
}

/// Accept one connection, then stall past the caller's deadline before
/// responding, so the acquisition deadline fires mid-read.
fn serve_after_delay(response: String, delay: Duration) -> Url {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        thread::sleep(delay);
        let _ = stream.write_all(response.as_bytes());
    });
    Url::parse(&format!("http://{address}/article")).unwrap()
}

/// Mirror `Reader::read_url` while keeping the `Execution` (cost ledger and
/// stage records) visible for effect assertions.
async fn execute_with_policy(url: &Url, policy: UrlPolicy) -> Execution {
    let mut request = ReadRequest::url(url.clone());
    request.url_policy = policy;
    Reader::new().execute(request).await
}

fn expect_failure(execution: &Execution) -> ReadError {
    match &execution.outcome {
        ExecutionOutcome::Failure(error) => error.clone(),
        ExecutionOutcome::Success(_) => panic!("expected a failed execution"),
    }
}

#[tokio::test]
async fn origin_http_obeys_explicit_private_network_policy() {
    let html = "<html><body><article><h1>Local test</h1><p>REQUIRED-HTTP: origin bytes are extracted locally after acquisition.</p></article></body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    let url = serve_once(response);

    let denied = Reader::new()
        .read_url(&url, &UrlPolicy::default())
        .await
        .unwrap_err();
    assert_eq!(denied.kind, ErrorKind::InvalidInput);
    // The refusal happens during network validation, before any socket work.
    assert_eq!(denied.stage, Stage::Validate);
    assert!(
        denied.message.contains("denied by default"),
        "{}",
        denied.message
    );

    let html = "<html><body><article><h1>Local test</h1><p>REQUIRED-HTTP: origin bytes are extracted locally after acquisition.</p></article></body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    let url = serve_once(response);
    let policy = UrlPolicy {
        allow_private_networks: true,
        ..UrlPolicy::default()
    };
    let article = Reader::new().read_url(&url, &policy).await.unwrap();
    assert!(article.content.contains("REQUIRED-HTTP"));
    assert_eq!(
        article.provenance.backend,
        readabilities_rs::Backend::Origin
    );
    assert_eq!(article.provenance.source_url.as_deref(), Some(url.as_str()));
    assert_eq!(
        article.provenance.snapshot,
        SnapshotObservations::origin_response()
    );
    assert!(!article.provenance.degraded);
}

#[tokio::test]
async fn origin_http_rejects_declared_body_above_budget() {
    let url = serve_once(
        "HTTP/1.1 200 OK\r\nContent-Length: 999999\r\nConnection: close\r\n\r\nsmall".to_string(),
    );
    let mut policy = UrlPolicy {
        allow_private_networks: true,
        ..UrlPolicy::default()
    };
    policy.budget.max_download_bytes = 128;
    let error = Reader::new().read_url(&url, &policy).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::BudgetExceeded);
    assert_eq!(error.retry, RetryAdvice::IncreaseBudget);
    assert_eq!(error.stage, Stage::Acquire);
    assert_eq!(
        error.message,
        "Content-Length 999999 exceeds byte budget 128"
    );
}

#[tokio::test]
async fn origin_http_decodes_declared_non_utf8_html_before_extraction() {
    let mut html = b"<html><body><article><h1>Encoding</h1><p>".to_vec();
    html.extend_from_slice(b"REQUIRED-CHARSET: caf\xe9 content remains readable after decoding with the declared legacy character encoding and native extraction.");
    html.extend_from_slice(b"</p></article></body></html>");
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=iso-8859-1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        html.len()
    );
    let mut response = header.into_bytes();
    response.extend_from_slice(&html);
    let url = serve_bytes(response);
    let policy = UrlPolicy {
        allow_private_networks: true,
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let article = match &execution.outcome {
        ExecutionOutcome::Success(article) => article,
        ExecutionOutcome::Failure(error) => panic!("the declared encoding must decode: {error}"),
    };
    assert!(article.content.contains("caf\u{e9}"), "{}", article.content);
    assert!(!article.content.contains('\u{fffd}'), "{}", article.content);
    // The acquire stage records which legacy encoding actually decoded the body.
    let acquire = execution
        .stages
        .iter()
        .find(|stage| stage.stage == Stage::Acquire)
        .expect("the acquire stage must be recorded");
    assert_eq!(
        acquire.detail,
        "origin HTML fetched within redirect, byte, and deadline limits; decoded as windows-1252"
    );
    // The full legacy body is charged once it is read to EOF.
    assert_eq!(execution.cost.downloaded_bytes, html.len());
    assert_eq!(execution.cost.origin_requests, 1);
}

fn undelimited_html_body(prefix_marker: &str, tail_marker: &str) -> Vec<u8> {
    // No Content-Length: the body length is only learned while streaming, so
    // the overrun happens mid-stream instead of at the declared-length gate.
    let mut body = b"<html><body><article><h1>Overrun</h1><p>REQUIRED-HTTP: ".to_vec();
    body.extend_from_slice(prefix_marker.as_bytes());
    body.extend_from_slice(b"</p><p>");
    body.extend_from_slice(&[b'x'; 4096]);
    body.extend_from_slice(b"</p><p>TAIL-MARKER: ");
    body.extend_from_slice(tail_marker.as_bytes());
    body.extend_from_slice(b"</p></article></body></html>");
    let mut response =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n".to_vec();
    response.extend_from_slice(&body);
    response
}

#[tokio::test]
async fn absent_content_type_falls_back_to_the_in_body_meta_charset() {
    // No Content-Type header at all: the encoding must come from the first
    // 2 KiB of the body itself, not from a default UTF-8 guess.
    let mut html = b"<html><head><meta charset=\"iso-8859-1\"></head><body><article><h1>Encoding</h1><p>REQUIRED-HTTP: caf\xe9 decoded from the in-body declaration.</p></article></body></html>".to_vec();
    let body_len = html.len();
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body_len
    )
    .into_bytes();
    response.append(&mut html);
    let url = serve_bytes(response);
    let policy = UrlPolicy {
        allow_private_networks: true,
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let article = match &execution.outcome {
        ExecutionOutcome::Success(article) => article,
        ExecutionOutcome::Failure(error) => panic!("the meta charset must decode: {error}"),
    };
    assert!(
        article.content.contains("REQUIRED-HTTP"),
        "{}",
        article.content
    );
    assert!(article.content.contains("caf\u{e9}"), "{}", article.content);
    assert!(!article.content.contains('\u{fffd}'), "{}", article.content);
    // The acquire record names the encoding the meta declaration resolved to.
    let acquire = execution
        .stages
        .iter()
        .find(|stage| stage.stage == Stage::Acquire)
        .expect("the acquire stage must be recorded");
    assert_eq!(
        acquire.detail,
        "origin HTML fetched within redirect, byte, and deadline limits; decoded as windows-1252"
    );
    assert_eq!(execution.cost.downloaded_bytes, body_len);
    assert_eq!(execution.cost.origin_requests, 1);
}

#[tokio::test]
async fn streaming_overrun_with_truncate_yields_bounded_prefix_and_warning() {
    let url = serve_bytes(undelimited_html_body(
        "bounded prefix survives",
        "dropped tail",
    ));
    let policy = UrlPolicy {
        allow_private_networks: true,
        truncate_overrun: true,
        budget: readabilities_rs::RequestBudget {
            max_download_bytes: 256,
            ..readabilities_rs::RequestBudget::default()
        },
        ..UrlPolicy::default()
    };

    let article = Reader::new().read_url(&url, &policy).await.unwrap();
    assert!(article.content.contains("bounded prefix survives"));
    assert!(!article.content.contains("dropped tail"));
    let truncated: Vec<_> = article
        .warnings
        .iter()
        .filter(|warning| warning.code == "byte_truncated")
        .collect();
    assert_eq!(
        truncated.len(),
        1,
        "truncation must be reported exactly once: {:?}",
        article.warnings
    );
    assert_eq!(
        truncated[0].message,
        "origin response exceeded the byte budget of 256 bytes; the body was cut to the budget mid-stream"
    );
}

#[tokio::test]
async fn exactly_at_cap_body_is_not_flagged_truncated() {
    let body = b"<html><body><article><h1>Capped</h1><p>REQUIRED-HTTP: an exactly-at-cap body reads to EOF and stays untruncated.</p></article></body></html>";
    let mut response =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n".to_vec();
    response.extend_from_slice(body);
    let url = serve_bytes(response);
    let policy = UrlPolicy {
        allow_private_networks: true,
        truncate_overrun: true,
        budget: readabilities_rs::RequestBudget {
            max_download_bytes: body.len(),
            ..readabilities_rs::RequestBudget::default()
        },
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let article = match &execution.outcome {
        ExecutionOutcome::Success(article) => article,
        ExecutionOutcome::Failure(error) => panic!("an at-cap body must succeed: {error}"),
    };
    assert!(
        article
            .warnings
            .iter()
            .all(|warning| warning.code != "byte_truncated")
    );
    // EOF was reached without dropping a byte: the whole body was delivered.
    assert!(article.content.contains("REQUIRED-HTTP"));
    // A clean UTF-8 read within limits reports the untruncated detail.
    let acquire = execution
        .stages
        .iter()
        .find(|stage| stage.stage == Stage::Acquire)
        .expect("the acquire stage must be recorded");
    assert_eq!(
        acquire.detail,
        "origin HTML fetched within redirect, byte, and deadline limits"
    );
    // The read is charged exactly the cap, not the cap plus a probe byte.
    assert_eq!(execution.cost.downloaded_bytes, body.len());
    assert_eq!(execution.cost.origin_requests, 1);
}

#[tokio::test]
async fn streaming_overrun_without_truncate_keeps_the_strict_budget_contract() {
    let url = serve_bytes(undelimited_html_body("never delivered", "never delivered"));
    let policy = UrlPolicy {
        allow_private_networks: true,
        budget: readabilities_rs::RequestBudget {
            max_download_bytes: 256,
            ..readabilities_rs::RequestBudget::default()
        },
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let error = expect_failure(&execution);
    assert_eq!(error.kind, ErrorKind::BudgetExceeded);
    assert_eq!(error.retry, RetryAdvice::IncreaseBudget);
    assert_eq!(error.stage, Stage::Acquire);
    assert_eq!(error.message, "download exceeded byte budget 256");
    // The mid-stream rejection charges the request but never the partial
    // bytes: the counter only moves after a clean read to EOF.
    assert_eq!(execution.cost.origin_requests, 1);
    assert_eq!(execution.cost.downloaded_bytes, 0);
    assert_eq!(execution.stages.len(), 1);
    assert_eq!(
        execution.stages[0].detail,
        format!("acquisition attempt failed: {}", error.message)
    );
}

#[tokio::test]
async fn declared_overrun_still_rejects_even_in_truncate_mode() {
    // An honest Content-Length above the cap is a negotiated over-fetch, not
    // an under-reporting server: truncate mode must not soften it.
    let url = serve_once(
        "HTTP/1.1 200 OK\r\nContent-Length: 999999\r\nConnection: close\r\n\r\nsmall".to_string(),
    );
    let policy = UrlPolicy {
        allow_private_networks: true,
        truncate_overrun: true,
        budget: readabilities_rs::RequestBudget {
            max_download_bytes: 128,
            ..readabilities_rs::RequestBudget::default()
        },
        ..UrlPolicy::default()
    };

    let error = Reader::new().read_url(&url, &policy).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::BudgetExceeded);
    assert_eq!(error.retry, RetryAdvice::IncreaseBudget);
    assert_eq!(error.stage, Stage::Acquire);
    assert_eq!(
        error.message,
        "Content-Length 999999 exceeds byte budget 128"
    );
}

#[tokio::test]
async fn redirects_are_followed_manually_and_land_on_the_final_origin_url() {
    let body = "<html><body><article><h1>Moved</h1><p>REQUIRED-HTTP: the article is served at the redirect target.</p></article></body></html>";
    let (url, requests) = serve_sequence(vec![redirect_response("/moved"), html_response(body)]);
    let policy = UrlPolicy {
        allow_private_networks: true,
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let article = match &execution.outcome {
        ExecutionOutcome::Success(article) => article,
        ExecutionOutcome::Failure(error) => {
            panic!("the redirect chain must land on the article: {error}")
        }
    };
    assert!(article.content.contains("REQUIRED-HTTP"));

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(
        requests[0].starts_with("GET /article HTTP/1.1"),
        "{requests:?}"
    );
    assert!(
        requests[1].starts_with("GET /moved HTTP/1.1"),
        "{requests:?}"
    );
    let user_agent = format!(
        "user-agent: readabilities-rs/{} (+https://github.com/junix/readabilities-rs)",
        readabilities_rs::VERSION
    );
    for request in requests.iter() {
        assert!(
            request.to_ascii_lowercase().contains(&user_agent),
            "{request:?} must identify the tool"
        );
    }

    let final_url = url.join("/moved").unwrap();
    assert_eq!(
        article.provenance.source_url.as_deref(),
        Some(final_url.as_str())
    );
    // The followed hop is charged: two requests, one full body downloaded.
    assert_eq!(execution.cost.origin_requests, 2);
    assert_eq!(execution.cost.downloaded_bytes, body.len());
}

#[tokio::test]
async fn redirect_budget_exhaustion_is_a_budget_error_with_no_extra_request() {
    let (url, requests) = serve_sequence(vec![
        redirect_response("/hop1"),
        redirect_response("/hop2"),
        redirect_response("/hop3"),
    ]);
    let policy = UrlPolicy {
        allow_private_networks: true,
        budget: readabilities_rs::RequestBudget {
            max_redirects: 2,
            ..readabilities_rs::RequestBudget::default()
        },
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let error = expect_failure(&execution);
    assert_eq!(error.kind, ErrorKind::BudgetExceeded);
    assert_eq!(error.retry, RetryAdvice::IncreaseBudget);
    assert_eq!(error.stage, Stage::Acquire);
    assert_eq!(error.message, "redirect budget exhausted");
    // Three requests were sent (the initial one plus two followed hops); the
    // hop past the budget is never requested.
    assert_eq!(execution.cost.origin_requests, 3);
    assert_eq!(requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn origin_request_budget_gates_the_next_request_before_sending() {
    let (url, requests) = serve_sequence(vec![
        redirect_response("/moved"),
        html_response("<html><body><article><p>never reached</p></article></body></html>"),
    ]);
    let policy = UrlPolicy {
        allow_private_networks: true,
        budget: readabilities_rs::RequestBudget {
            max_origin_requests: 1,
            ..readabilities_rs::RequestBudget::default()
        },
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let error = expect_failure(&execution);
    assert_eq!(error.kind, ErrorKind::BudgetExceeded);
    assert_eq!(error.retry, RetryAdvice::IncreaseBudget);
    assert_eq!(error.stage, Stage::Acquire);
    assert_eq!(error.message, "origin request budget exhausted");
    assert_eq!(execution.cost.origin_requests, 1);
    // The redirect was accepted but never followed: no second connection.
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn redirect_without_a_location_header_is_rejected() {
    let (url, requests) = serve_sequence(vec![
        b"HTTP/1.1 302 Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    ]);
    let policy = UrlPolicy {
        allow_private_networks: true,
        ..UrlPolicy::default()
    };

    let error = Reader::new().read_url(&url, &policy).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::OriginHttp);
    assert_eq!(error.stage, Stage::Acquire);
    assert_eq!(
        error.message,
        "redirect response omitted a valid Location header"
    );
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn redirect_to_a_non_http_scheme_is_rejected() {
    let (url, requests) = serve_sequence(vec![redirect_response("ftp://127.0.0.1/files")]);
    let policy = UrlPolicy {
        allow_private_networks: true,
        ..UrlPolicy::default()
    };

    let error = Reader::new().read_url(&url, &policy).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::OriginHttp);
    assert_eq!(error.stage, Stage::Acquire);
    assert_eq!(error.message, "redirect target must use http or https");
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn redirect_with_embedded_credentials_is_rejected() {
    let (url, requests) = serve_sequence(vec![redirect_response(
        "http://alice:secret@127.0.0.1/landing",
    )]);
    let policy = UrlPolicy {
        allow_private_networks: true,
        ..UrlPolicy::default()
    };

    let error = Reader::new().read_url(&url, &policy).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::OriginHttp);
    assert_eq!(error.stage, Stage::Acquire);
    assert_eq!(
        error.message,
        "redirect target contains embedded credentials"
    );
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn cross_origin_redirect_is_denied_by_default_and_allowed_when_opted_in() {
    let body = "<html><body><article><h1>Elsewhere</h1><p>REQUIRED-HTTP: a different port is a different origin.</p></article></body></html>";

    // Denial: the default policy refuses to leave the initial origin.
    let (origin_b, b_requests) = serve_sequence(vec![html_response(body)]);
    let authority_b = format!(
        "http://{}:{}",
        origin_b.host_str().unwrap(),
        origin_b.port().unwrap()
    );
    let (url_a, a_requests) =
        serve_sequence(vec![redirect_response(&format!("{authority_b}/landing"))]);
    let error = Reader::new()
        .read_url(
            &url_a,
            &UrlPolicy {
                allow_private_networks: true,
                ..UrlPolicy::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::OriginHttp);
    assert_eq!(error.stage, Stage::Acquire);
    assert_eq!(error.message, "cross-origin redirect denied by policy");
    assert_eq!(error.retry, RetryAdvice::ChooseAnotherBackend);
    assert_eq!(a_requests.lock().unwrap().len(), 1);
    assert!(
        b_requests.lock().unwrap().is_empty(),
        "the cross-origin target must never be contacted"
    );

    // Opt-in: the same redirect is followed to the other origin.
    let (origin_b, b_requests) = serve_sequence(vec![html_response(body)]);
    let authority_b = format!(
        "http://{}:{}",
        origin_b.host_str().unwrap(),
        origin_b.port().unwrap()
    );
    let (url_a, a_requests) =
        serve_sequence(vec![redirect_response(&format!("{authority_b}/landing"))]);
    let policy = UrlPolicy {
        allow_private_networks: true,
        allow_cross_origin_redirects: true,
        ..UrlPolicy::default()
    };
    let article = Reader::new().read_url(&url_a, &policy).await.unwrap();
    assert!(article.content.contains("REQUIRED-HTTP"));
    let expected_landing = Url::parse(&format!("{authority_b}/landing")).unwrap();
    assert_eq!(
        article.provenance.source_url.as_deref(),
        Some(expected_landing.as_str())
    );
    assert_eq!(a_requests.lock().unwrap().len(), 1);
    assert_eq!(b_requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn http_status_failures_map_to_typed_error_kinds_and_retry_advice() {
    for (status, reason, kind, retry) in [
        (
            401_u16,
            "Unauthorized",
            ErrorKind::Authentication,
            RetryAdvice::Never,
        ),
        (
            403,
            "Forbidden",
            ErrorKind::Authentication,
            RetryAdvice::Never,
        ),
        (
            408,
            "Request Timeout",
            ErrorKind::RateLimit,
            RetryAdvice::RetryAfter,
        ),
        (
            429,
            "Too Many Requests",
            ErrorKind::RateLimit,
            RetryAdvice::RetryAfter,
        ),
        (
            500,
            "Internal Server Error",
            ErrorKind::OriginHttp,
            RetryAdvice::RetrySameBackend,
        ),
        (
            503,
            "Service Unavailable",
            ErrorKind::OriginHttp,
            RetryAdvice::RetrySameBackend,
        ),
        (404, "Not Found", ErrorKind::OriginHttp, RetryAdvice::Never),
        // Any other status falls through to the generic terminal mapping.
        (
            400,
            "Bad Request",
            ErrorKind::OriginHttp,
            RetryAdvice::Never,
        ),
        (410, "Gone", ErrorKind::OriginHttp, RetryAdvice::Never),
    ] {
        let (url, requests) = serve_sequence(vec![
            format!("HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .into_bytes(),
        ]);
        let policy = UrlPolicy {
            allow_private_networks: true,
            ..UrlPolicy::default()
        };

        let error = Reader::new().read_url(&url, &policy).await.unwrap_err();
        assert_eq!(error.kind, kind, "HTTP {status}");
        assert_eq!(error.retry, retry, "HTTP {status}");
        assert_eq!(error.stage, Stage::Acquire, "HTTP {status}");
        assert!(
            error
                .message
                .contains(&format!("origin returned HTTP {status}")),
            "HTTP {status}: {}",
            error.message
        );
        // A failing status is terminal for this acquisition: one request only.
        assert_eq!(requests.lock().unwrap().len(), 1, "HTTP {status}");
    }
}

#[tokio::test]
async fn deadline_exhaustion_is_a_timeout_advising_a_bigger_budget() {
    let body = "<html><body><article><p>REQUIRED-HTTP: never delivered in time.</p></article></body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let url = serve_after_delay(response, Duration::from_millis(500));
    let policy = UrlPolicy {
        allow_private_networks: true,
        budget: readabilities_rs::RequestBudget {
            deadline: Duration::from_millis(100),
            ..readabilities_rs::RequestBudget::default()
        },
        ..UrlPolicy::default()
    };

    let error = Reader::new().read_url(&url, &policy).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::Timeout);
    assert_eq!(error.retry, RetryAdvice::IncreaseBudget);
    assert_eq!(error.stage, Stage::Acquire);
    // Which timeout layer reports first is a scheduling race, so only the
    // typed contract above is pinned, not the message wording.
}

#[tokio::test]
async fn zero_download_budget_is_rejected_before_any_request() {
    // Port 9 (discard) has nothing listening: an InvalidInput here proves the
    // budget was validated before any connection was attempted.
    let url = Url::parse("http://127.0.0.1:9/article").unwrap();
    let policy = UrlPolicy {
        budget: readabilities_rs::RequestBudget {
            max_download_bytes: 0,
            ..readabilities_rs::RequestBudget::default()
        },
        ..UrlPolicy::default()
    };

    let error = Reader::new().read_url(&url, &policy).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidInput);
    assert_eq!(error.stage, Stage::Validate);
    assert_eq!(
        error.message,
        "deadline and max_download_bytes must be positive"
    );
}

#[tokio::test]
async fn non_http_url_schemes_are_rejected_before_any_request() {
    let url = Url::parse("ftp://example.test/article").unwrap();
    let error = Reader::new()
        .read_url(&url, &UrlPolicy::default())
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidInput);
    assert_eq!(error.stage, Stage::Validate);
    assert_eq!(
        error.message,
        "URL acquisition only supports http and https"
    );
}

#[tokio::test]
async fn truncation_charges_exactly_the_budgeted_bytes_and_records_the_stage() {
    let url = serve_bytes(undelimited_html_body(
        "bounded prefix survives",
        "dropped tail",
    ));
    let policy = UrlPolicy {
        allow_private_networks: true,
        truncate_overrun: true,
        budget: readabilities_rs::RequestBudget {
            max_download_bytes: 256,
            ..readabilities_rs::RequestBudget::default()
        },
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let article = match &execution.outcome {
        ExecutionOutcome::Success(article) => article,
        ExecutionOutcome::Failure(error) => panic!("expected truncation to succeed: {error}"),
    };
    assert!(article.content.contains("bounded prefix survives"));
    // The usable prefix is charged exactly: one request, cap-sized download.
    assert_eq!(execution.cost.origin_requests, 1);
    assert_eq!(execution.cost.downloaded_bytes, 256);
    let acquire = execution
        .stages
        .iter()
        .find(|stage| stage.stage == Stage::Acquire)
        .expect("the acquire stage must be recorded");
    assert_eq!(
        acquire.detail,
        "origin body exceeded the byte budget and was truncated to 256 bytes"
    );
}

#[tokio::test]
async fn connection_refusal_is_an_origin_http_error_charging_exactly_one_request() {
    // Port 9 (discard) accepts nothing: the TCP connect itself fails, so the
    // transport-failure mapping is exercised without a server.
    let url = Url::parse("http://127.0.0.1:9/article").unwrap();
    let policy = UrlPolicy {
        allow_private_networks: true,
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let error = expect_failure(&execution);
    assert_eq!(error.kind, ErrorKind::OriginHttp);
    assert_eq!(error.stage, Stage::Acquire);
    // Transport failures are retryable on the same backend.
    assert_eq!(error.retry, RetryAdvice::RetrySameBackend);
    assert!(
        error.message.contains("error sending request"),
        "{}",
        error.message
    );
    // The attempt was charged, but no bytes were ever downloaded.
    assert_eq!(execution.cost.origin_requests, 1);
    assert_eq!(execution.cost.downloaded_bytes, 0);
    // The failed attempt is ledgered with its cause before extraction runs.
    assert_eq!(execution.stages.len(), 1);
    assert_eq!(execution.stages[0].stage, Stage::Acquire);
    assert_eq!(
        execution.stages[0].detail,
        format!("acquisition attempt failed: {}", error.message)
    );
}

#[tokio::test]
async fn unparseable_redirect_location_is_rejected_without_a_second_request() {
    let (url, requests) = serve_sequence(vec![redirect_response("http://[")]);
    let policy = UrlPolicy {
        allow_private_networks: true,
        ..UrlPolicy::default()
    };

    let error = Reader::new().read_url(&url, &policy).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::OriginHttp);
    assert_eq!(error.stage, Stage::Acquire);
    assert_eq!(
        error.message,
        "invalid redirect target: invalid IPv6 address"
    );
    // The unusable target is never contacted.
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn invalid_utf8_bodies_report_replaced_sequences_in_the_stage_detail() {
    let mut html = b"<html><body><article><h1>Encoding</h1><p>REQUIRED-HTTP: ".to_vec();
    html.extend_from_slice(b"caf\xe9 and an invalid \xff byte");
    html.extend_from_slice(b" survive decoding lossily.</p></article></body></html>");
    let mut response =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n"
            .to_vec();
    response.extend_from_slice(&html);
    let url = serve_bytes(response);
    let policy = UrlPolicy {
        allow_private_networks: true,
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let article = match &execution.outcome {
        ExecutionOutcome::Success(article) => article,
        ExecutionOutcome::Failure(error) => panic!("lossy decoding must still extract: {error}"),
    };
    assert!(article.content.contains("REQUIRED-HTTP"));
    assert!(article.content.contains('\u{fffd}'), "{}", article.content);
    // Undecodable sequences are surfaced on the acquire record, not swallowed.
    let acquire = execution
        .stages
        .iter()
        .find(|stage| stage.stage == Stage::Acquire)
        .expect("the acquire stage must be recorded");
    assert_eq!(
        acquire.detail,
        "origin HTML fetched within limits; invalid UTF-8 sequences were replaced"
    );
}

#[tokio::test]
async fn localhost_hostnames_resolve_and_remain_denied_by_default() {
    // A DNS name (not a literal) still routes through network validation:
    // resolving to a loopback address keeps the default deny.
    let url = Url::parse("http://localhost/article").unwrap();

    let execution = Reader::new().execute(ReadRequest::url(url)).await;
    let error = expect_failure(&execution);
    assert_eq!(error.kind, ErrorKind::InvalidInput);
    assert_eq!(error.stage, Stage::Validate);
    assert!(
        error.message.contains("denied by default"),
        "{}",
        error.message
    );
    // The refusal is ledgered as one failed acquisition attempt that never
    // downloaded a byte.
    assert_eq!(execution.cost.origin_requests, 1);
    assert_eq!(execution.cost.downloaded_bytes, 0);
    assert_eq!(execution.stages.len(), 1);
    assert_eq!(execution.stages[0].stage, Stage::Acquire);
    assert_eq!(
        execution.stages[0].detail,
        format!("acquisition attempt failed: {}", error.message)
    );
}

#[tokio::test]
async fn explicit_fallback_success_is_degraded_and_fully_ledgered() {
    let html = "<html><body><article><h1>Fallback</h1><p>REQUIRED-HTTP: the explicitly authorized fallback acquisition delivers the article.</p></article></body></html>";
    let (url, requests) = serve_sequence(vec![
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        html_response(html),
    ]);
    let policy = UrlPolicy {
        allow_private_networks: true,
        fallbacks: vec![readabilities_rs::Acquisition::Origin],
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let article = match &execution.outcome {
        ExecutionOutcome::Success(article) => article,
        ExecutionOutcome::Failure(error) => panic!("the fallback must deliver: {error}"),
    };
    assert!(article.content.contains("REQUIRED-HTTP"));
    // A fallback result is never presented as a pristine origin read.
    assert!(article.provenance.degraded);
    let fallback_warnings: Vec<_> = article
        .warnings
        .iter()
        .filter(|warning| warning.code == "acquisition_fallback")
        .collect();
    assert_eq!(
        fallback_warnings.len(),
        1,
        "the fallback must be reported exactly once: {:?}",
        article.warnings
    );
    assert_eq!(
        fallback_warnings[0].message,
        "selected explicit fallback acquisition origin"
    );

    // Both requests were spent; only the successful body is charged.
    assert_eq!(execution.cost.origin_requests, 2);
    assert_eq!(execution.cost.downloaded_bytes, html.len());
    assert_eq!(requests.lock().unwrap().len(), 2);

    // The failed primary and the selected fallback are ledgered in order.
    assert_eq!(execution.attempts[0].engine, "acquire:origin");
    assert!(!execution.attempts[0].selected);
    assert_eq!(
        execution.attempts[0].failure.as_deref(),
        Some("origin returned HTTP 404 Not Found")
    );
    assert_eq!(execution.attempts[1].engine, "acquire:origin");
    assert!(execution.attempts[1].selected);
    assert_eq!(execution.attempts[1].failure, None);
    assert_eq!(
        execution.stages[0].detail,
        "acquisition attempt failed: origin returned HTTP 404 Not Found"
    );
    assert_eq!(
        execution.stages[1].detail,
        "origin HTML fetched within redirect, byte, and deadline limits"
    );
}

#[tokio::test]
async fn exhausted_fallbacks_surface_the_last_error_with_the_complete_ledger() {
    let (url, requests) = serve_sequence(vec![
        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            .to_vec(),
        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            .to_vec(),
    ]);
    let policy = UrlPolicy {
        allow_private_networks: true,
        fallbacks: vec![readabilities_rs::Acquisition::Origin],
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let error = expect_failure(&execution);
    assert_eq!(error.kind, ErrorKind::OriginHttp);
    assert_eq!(error.retry, RetryAdvice::RetrySameBackend);
    assert_eq!(
        error.message,
        "origin returned HTTP 500 Internal Server Error"
    );
    // Every acquisition attempt is ledgered; no extraction attempt ever ran.
    assert_eq!(execution.attempts.len(), 2);
    assert!(
        execution
            .attempts
            .iter()
            .all(|attempt| attempt.engine == "acquire:origin"
                && !attempt.selected
                && attempt.failure.as_deref()
                    == Some("origin returned HTTP 500 Internal Server Error"))
    );
    assert_eq!(execution.stages.len(), 2);
    assert!(execution
        .stages
        .iter()
        .all(|stage| stage.stage == Stage::Acquire
            && stage.detail
                == "acquisition attempt failed: origin returned HTTP 500 Internal Server Error"));
    // The terminal error carries the completed attempts for callers that only
    // take the Result.
    assert_eq!(error.completed_attempts.len(), 2);
    assert_eq!(execution.cost.origin_requests, 2);
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn declared_length_at_the_cap_passes_and_one_byte_over_is_rejected() {
    // The declared gate is strictly greater-than: an honest Content-Length
    // exactly at the budget must sail through, while one extra byte is
    // refused before a single body byte is read.
    let body = "<html><body><article><p>REQUIRED-HTTP: exactly the negotiated size.</p></article></body></html>";
    let at_cap = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let url = serve_once(at_cap);
    let policy = UrlPolicy {
        allow_private_networks: true,
        budget: readabilities_rs::RequestBudget {
            max_download_bytes: body.len(),
            ..readabilities_rs::RequestBudget::default()
        },
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let article = match &execution.outcome {
        ExecutionOutcome::Success(article) => article,
        ExecutionOutcome::Failure(error) => panic!("an at-cap declared length must pass: {error}"),
    };
    assert!(article.content.contains("REQUIRED-HTTP"));
    assert!(
        article
            .warnings
            .iter()
            .all(|warning| warning.code != "byte_truncated")
    );
    assert_eq!(execution.cost.downloaded_bytes, body.len());
    assert_eq!(execution.cost.origin_requests, 1);

    // One byte over the cap: refused at the declared-length gate, so no body
    // byte is ever read or charged.
    let over = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}x",
        body.len() + 1
    );
    let url = serve_once(over);
    let policy = UrlPolicy {
        allow_private_networks: true,
        budget: readabilities_rs::RequestBudget {
            max_download_bytes: body.len(),
            ..readabilities_rs::RequestBudget::default()
        },
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let error = expect_failure(&execution);
    assert_eq!(error.kind, ErrorKind::BudgetExceeded);
    assert_eq!(error.retry, RetryAdvice::IncreaseBudget);
    assert_eq!(error.stage, Stage::Acquire);
    assert_eq!(
        error.message,
        format!(
            "Content-Length {} exceeds byte budget {}",
            body.len() + 1,
            body.len()
        )
    );
    // The refused request is charged, but not one byte of the body.
    assert_eq!(execution.cost.origin_requests, 1);
    assert_eq!(execution.cost.downloaded_bytes, 0);
    assert_eq!(execution.stages.len(), 1);
    assert_eq!(
        execution.stages[0].detail,
        format!("acquisition attempt failed: {}", error.message)
    );
}

#[tokio::test]
async fn prematurely_closed_body_is_a_transport_error_charging_no_bytes() {
    // The server declares 512 bytes but closes after a short prefix: the
    // stream errors mid-read and the partial bytes must never be charged.
    let url = serve_bytes(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 512\r\nConnection: close\r\n\r\nshort prefix"
            .to_vec(),
    );
    let policy = UrlPolicy {
        allow_private_networks: true,
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let error = expect_failure(&execution);
    assert_eq!(error.kind, ErrorKind::OriginHttp);
    assert_eq!(error.retry, RetryAdvice::RetrySameBackend);
    assert_eq!(error.stage, Stage::Acquire);
    assert!(
        error.message.contains("error decoding response body"),
        "{}",
        error.message
    );
    // The attempt is ledgered, but the partial prefix is not charged: the
    // byte counter only moves after a clean read to EOF.
    assert_eq!(execution.cost.origin_requests, 1);
    assert_eq!(execution.cost.downloaded_bytes, 0);
    assert_eq!(execution.stages.len(), 1);
    assert_eq!(
        execution.stages[0].detail,
        format!("acquisition attempt failed: {}", error.message)
    );
}

#[tokio::test]
async fn error_status_bodies_are_rejected_without_being_downloaded() {
    // A 500 with a real body is terminal at the status gate: the body is
    // never read, so not one of its bytes lands in the ledger.
    let (url, _requests) = serve_sequence(vec![
        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 24\r\nConnection: close\r\n\r\ninternal failure detail"
            .to_vec(),
    ]);
    let policy = UrlPolicy {
        allow_private_networks: true,
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let error = expect_failure(&execution);
    assert_eq!(error.kind, ErrorKind::OriginHttp);
    assert_eq!(error.retry, RetryAdvice::RetrySameBackend);
    assert_eq!(error.stage, Stage::Acquire);
    assert_eq!(
        error.message,
        "origin returned HTTP 500 Internal Server Error"
    );
    assert_eq!(execution.cost.origin_requests, 1);
    assert_eq!(execution.cost.downloaded_bytes, 0);
}

#[tokio::test]
async fn an_empty_200_body_fails_extraction_with_the_acquisition_still_ledgered() {
    // Acquisition succeeds on an empty body; the failure belongs to the
    // extraction stage, and the successful fetch must remain in the ledger.
    let (url, requests) = serve_sequence(vec![
        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    ]);
    let policy = UrlPolicy {
        allow_private_networks: true,
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let error = expect_failure(&execution);
    assert_eq!(error.kind, ErrorKind::InvalidInput);
    assert_eq!(error.stage, Stage::Validate);
    assert_eq!(error.message, "HTML input is empty");
    // The fetch itself succeeded: one request, zero body bytes, and the
    // acquire stage records the clean read that produced the empty body.
    assert_eq!(execution.cost.origin_requests, 1);
    assert_eq!(execution.cost.downloaded_bytes, 0);
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert_eq!(execution.stages.len(), 1);
    assert_eq!(execution.stages[0].stage, Stage::Acquire);
    assert_eq!(
        execution.stages[0].detail,
        "origin HTML fetched within redirect, byte, and deadline limits"
    );
    // The acquisition attempt is kept, but marked unselected with the
    // extraction failure as its cause.
    assert_eq!(execution.attempts.len(), 1);
    assert_eq!(execution.attempts[0].engine, "acquire:origin");
    assert!(!execution.attempts[0].selected);
    assert_eq!(
        execution.attempts[0].failure.as_deref(),
        Some("HTML input is empty")
    );
    assert_eq!(error.completed_attempts.len(), 1);
}

#[tokio::test]
async fn the_last_allowed_redirect_hop_may_still_land_the_article() {
    // Spending the entire redirect budget is legal: the gate only fires when
    // the response AFTER the final allowed hop is itself a redirect.
    let body = "<html><body><article><p>REQUIRED-HTTP: landed on the last allowed hop.</p></article></body></html>";
    let (url, requests) = serve_sequence(vec![
        redirect_response("/hop1"),
        redirect_response("/hop2"),
        html_response(body),
    ]);
    let policy = UrlPolicy {
        allow_private_networks: true,
        budget: readabilities_rs::RequestBudget {
            max_redirects: 2,
            ..readabilities_rs::RequestBudget::default()
        },
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let article = match &execution.outcome {
        ExecutionOutcome::Success(article) => article,
        ExecutionOutcome::Failure(error) => panic!("the last hop must land: {error}"),
    };
    assert!(article.content.contains("REQUIRED-HTTP"));
    let final_url = url.join("/hop2").unwrap();
    assert_eq!(
        article.provenance.source_url.as_deref(),
        Some(final_url.as_str())
    );
    // The whole chain was spent and charged.
    assert_eq!(requests.lock().unwrap().len(), 3);
    assert_eq!(execution.cost.origin_requests, 3);
    assert_eq!(execution.cost.downloaded_bytes, body.len());
}

#[tokio::test]
async fn a_zero_redirect_budget_rejects_the_first_redirect_without_following() {
    // The redirect budget counts followed hops: with a budget of zero, the
    // very first redirect response is refused and never dereferenced.
    let (url, requests) = serve_sequence(vec![
        redirect_response("/moved"),
        html_response("<html><body><article><p>never reached</p></article></body></html>"),
    ]);
    let policy = UrlPolicy {
        allow_private_networks: true,
        budget: readabilities_rs::RequestBudget {
            max_redirects: 0,
            ..readabilities_rs::RequestBudget::default()
        },
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let error = expect_failure(&execution);
    assert_eq!(error.kind, ErrorKind::BudgetExceeded);
    assert_eq!(error.retry, RetryAdvice::IncreaseBudget);
    assert_eq!(error.stage, Stage::Acquire);
    assert_eq!(error.message, "redirect budget exhausted");
    // The initial request was sent and charged; the target was not followed.
    assert_eq!(execution.cost.origin_requests, 1);
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_single_request_succeeds_at_the_origin_request_budget_cap() {
    // The request gate is `spent >= max`: a budget of one must still admit
    // the very first request, not reject it before it is sent.
    let body = "<html><body><article><p>REQUIRED-HTTP: one request, one budget slot.</p></article></body></html>";
    let (url, requests) = serve_sequence(vec![html_response(body)]);
    let policy = UrlPolicy {
        allow_private_networks: true,
        budget: readabilities_rs::RequestBudget {
            max_origin_requests: 1,
            ..readabilities_rs::RequestBudget::default()
        },
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let article = match &execution.outcome {
        ExecutionOutcome::Success(article) => article,
        ExecutionOutcome::Failure(error) => {
            panic!("a budget of one must admit one request: {error}")
        }
    };
    assert!(article.content.contains("REQUIRED-HTTP"));
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert_eq!(execution.cost.origin_requests, 1);
    assert_eq!(execution.cost.downloaded_bytes, body.len());
}

#[tokio::test]
async fn dns_failure_for_a_domain_is_one_retryable_charged_attempt() {
    // `.invalid` never resolves (RFC 6761): the lookup error must surface as
    // a typed, retryable acquisition failure with no bytes downloaded.
    let url = Url::parse("http://readabilities-nonexistent.invalid/article").unwrap();
    let policy = UrlPolicy {
        allow_private_networks: true,
        ..UrlPolicy::default()
    };

    let execution = execute_with_policy(&url, policy).await;
    let error = expect_failure(&execution);
    assert_eq!(error.kind, ErrorKind::OriginHttp);
    assert_eq!(error.retry, RetryAdvice::RetrySameBackend);
    assert_eq!(error.stage, Stage::Acquire);
    assert!(
        error.message.starts_with("DNS lookup failed:"),
        "{}",
        error.message
    );
    // The request slot was already spent when the lookup failed.
    assert_eq!(execution.cost.origin_requests, 1);
    assert_eq!(execution.cost.downloaded_bytes, 0);
    assert_eq!(execution.stages.len(), 1);
    assert_eq!(
        execution.stages[0].detail,
        format!("acquisition attempt failed: {}", error.message)
    );
}

#[tokio::test]
async fn zero_deadline_is_rejected_before_any_request() {
    // The other operand of the budget precondition: a zero deadline is
    // refused up front. Port 9 (discard) has nothing listening, so an
    // InvalidInput here proves validation preceded any connection.
    let url = Url::parse("http://127.0.0.1:9/article").unwrap();
    let policy = UrlPolicy {
        budget: readabilities_rs::RequestBudget {
            deadline: Duration::ZERO,
            ..readabilities_rs::RequestBudget::default()
        },
        ..UrlPolicy::default()
    };

    let error = Reader::new().read_url(&url, &policy).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidInput);
    assert_eq!(error.stage, Stage::Validate);
    assert_eq!(
        error.message,
        "deadline and max_download_bytes must be positive"
    );
}
