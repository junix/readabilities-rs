#![cfg(feature = "http")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use readabilities_rs::{ErrorKind, Reader, UrlPolicy};
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

    let article = Reader::new().read_url(&url, &policy).await.unwrap();
    assert!(article.content.contains("caf\u{e9}"), "{}", article.content);
    assert!(!article.content.contains('\u{fffd}'), "{}", article.content);
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
    let warning = article
        .warnings
        .iter()
        .find(|warning| warning.code == "byte_truncated")
        .expect("truncated body must carry the byte_truncated warning");
    assert!(warning.message.contains("256"));
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

    let article = Reader::new().read_url(&url, &policy).await.unwrap();
    assert!(
        article
            .warnings
            .iter()
            .all(|warning| warning.code != "byte_truncated")
    );
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

    let error = Reader::new().read_url(&url, &policy).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::BudgetExceeded);
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
}
