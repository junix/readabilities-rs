#![cfg(feature = "http")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use readabilities_rs::{ErrorKind, Reader, UrlPolicy};
use url::Url;

fn serve_once(response: String) -> Url {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        stream.write_all(response.as_bytes()).unwrap();
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
