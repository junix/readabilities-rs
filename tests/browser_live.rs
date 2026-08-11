#![cfg(feature = "browser")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use readabilities_rs::{Acquisition, BrowserPolicy, ReadRequest, Reader, SnapshotKind};
use url::Url;

fn serve_js_shell() -> Url {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        let html = r#"<!doctype html><html><head><title>JS shell</title></head>
          <body><div id="root">LOADING-SHELL</div><script>
          document.getElementById('root').innerHTML =
            '<article><h1>Rendered article</h1><p>REQUIRED-BROWSER-JS: live DOM content was rendered by JavaScript.</p></article>';
          </script></body></html>"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
            html.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    Url::parse(&format!("http://{address}/article")).unwrap()
}

#[tokio::test]
#[ignore = "requires a locally installed Chrome/Chromium executable"]
async fn browser_acquires_javascript_rendered_live_dom() {
    let mut request = ReadRequest::url(serve_js_shell());
    request.url_policy.allow_private_networks = true;
    request.url_policy.budget.deadline = Duration::from_secs(20);
    request.url_policy.acquisition = Acquisition::Browser(BrowserPolicy {
        wait_after_load: Duration::from_millis(200),
        ..BrowserPolicy::default()
    });

    let article = Reader::new().execute(request).await.into_result().unwrap();
    assert!(article.content.contains("REQUIRED-BROWSER-JS"));
    assert!(!article.content.contains("LOADING-SHELL"));
    assert_eq!(article.provenance.snapshot.kind, SnapshotKind::BrowserDom);
    assert!(article.provenance.snapshot.javascript_executed);
    assert!(!article.provenance.snapshot.computed_styles);
}
