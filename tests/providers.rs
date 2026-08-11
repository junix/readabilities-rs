#![cfg(feature = "providers")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

use readabilities_rs::{
    Acquisition, CostValue, ExecutionOutcome, FirecrawlConfig, JinaConfig, ManagedProvider,
    ReadRequest, Reader, YxtConfig,
};
use secrecy::SecretString;
use url::Url;

fn serve_json_once(json: &'static str) -> (Url, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            let request_text = String::from_utf8_lossy(&request);
            let header_end = request_text.find("\r\n\r\n");
            if let Some(header_end) = header_end {
                let length = request_text[..header_end]
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length: ")
                            .or_else(|| line.strip_prefix("Content-Length: "))
                    })
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if request.len() >= header_end + 4 + length {
                    break;
                }
            }
        }
        sender
            .send(String::from_utf8_lossy(&request).into_owned())
            .unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
            json.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    (
        Url::parse(&format!("http://{address}/v2/scrape")).unwrap(),
        receiver,
    )
}

fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = stream.read(&mut buffer).unwrap();
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        let request_text = String::from_utf8_lossy(&request);
        if let Some(header_end) = request_text.find("\r\n\r\n") {
            let length = request_text[..header_end]
                .lines()
                .find_map(|line| {
                    let lowercase = line.to_ascii_lowercase();
                    lowercase
                        .strip_prefix("content-length: ")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if request.len() >= header_end + 4 + length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&request).into_owned()
}

fn send_json(stream: &mut std::net::TcpStream, json: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
        json.len()
    );
    stream.write_all(response.as_bytes()).unwrap();
}

#[tokio::test]
async fn firecrawl_adapter_is_explicit_bounded_and_sanitized() {
    let response = r#"{
      "success": true,
      "data": {
        "html": "<article><h1>Managed result</h1><p>REQUIRED-PROVIDER: returned HTML still crosses the local sanitizer.</p><script>bad()</script></article>",
        "metadata": {"title":"Provider title","description":"Provider description","language":"en","sourceURL":"https://example.test/provider"}
      }
    }"#;
    let (endpoint, captured) = serve_json_once(response);
    let target = Url::parse(&endpoint.as_str().replace("/v2/scrape", "/target")).unwrap();
    let mut config = FirecrawlConfig::new(SecretString::from("test-secret"));
    config.endpoint = endpoint;
    let mut request = ReadRequest::url(target);
    request.url_policy.allow_private_networks = true;
    request.url_policy.acquisition = Acquisition::Managed(ManagedProvider::Firecrawl(config));

    let execution = Reader::new().execute(request).await;
    let article = match &execution.outcome {
        ExecutionOutcome::Success(article) => article,
        ExecutionOutcome::Failure(error) => panic!("provider failed: {error}"),
    };
    assert!(article.content.contains("REQUIRED-PROVIDER"));
    assert!(!article.content.contains("<script"));
    assert_eq!(article.metadata.title.as_deref(), Some("Provider title"));
    assert_eq!(execution.cost.billable_submissions, 1);
    assert!(matches!(execution.cost.monetary_cost, CostValue::Unknown));

    let request = captured.recv().unwrap();
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer test-secret")
    );
    assert!(request.contains("\"onlyMainContent\":true"));
    assert!(request.contains("\"formats\":[\"markdown\",\"html\"]"));
}

#[tokio::test]
async fn zero_billable_budget_prevents_provider_submission() {
    let target = Url::parse("http://127.0.0.1:9/target").unwrap();
    let mut config = FirecrawlConfig::new(SecretString::from("never-sent"));
    config.endpoint = Url::parse("http://127.0.0.1:9/v2/scrape").unwrap();
    let mut request = ReadRequest::url(target);
    request.url_policy.allow_private_networks = true;
    request.url_policy.budget.max_billable_submissions = 0;
    request.url_policy.acquisition = Acquisition::Managed(ManagedProvider::Firecrawl(config));
    let execution = Reader::new().execute(request).await;
    assert!(matches!(
        execution.outcome,
        ExecutionOutcome::Failure(readabilities_rs::ReadError {
            kind: readabilities_rs::ErrorKind::BudgetExceeded,
            ..
        })
    ));
    assert_eq!(execution.cost.billable_submissions, 0);
}

#[tokio::test]
async fn jina_adapter_converts_native_markdown_and_merges_metadata() {
    let response = r##"{
      "data": {
        "title": "Jina title",
        "description": "Jina description",
        "url": "https://example.test/jina",
        "content": "# Managed heading\n\nREQUIRED-JINA: native Markdown becomes canonical sanitized HTML.\n\n<script>bad()</script>"
      }
    }"##;
    let (endpoint, captured) = serve_json_once(response);
    let target = Url::parse(&endpoint.as_str().replace("/v2/scrape", "/target")).unwrap();
    let config = JinaConfig {
        endpoint: Url::parse(&format!(
            "{}/",
            endpoint.as_str().trim_end_matches("/v2/scrape")
        ))
        .unwrap(),
        api_key: Some(SecretString::from("jina-test-key")),
        ..JinaConfig::default()
    };
    let mut request = ReadRequest::url(target);
    request.url_policy.allow_private_networks = true;
    request.url_policy.acquisition = Acquisition::Managed(ManagedProvider::Jina(config));
    let execution = Reader::new().execute(request).await;
    let article = match execution.outcome {
        ExecutionOutcome::Success(article) => article,
        ExecutionOutcome::Failure(error) => panic!("Jina failed: {error}"),
    };
    assert!(article.content.contains("REQUIRED-JINA"));
    assert!(!article.content.contains("<script"));
    assert_eq!(article.metadata.title.as_deref(), Some("Jina title"));
    assert!(article.provenance.markdown_native);
    let request = captured.recv().unwrap().to_ascii_lowercase();
    assert!(request.contains("x-respond-with: markdown"));
    assert!(request.contains("authorization: bearer jina-test-key"));
}

#[tokio::test]
async fn explicitly_authorized_fallback_is_visible_and_degraded() {
    let response = r##"{
      "data": {
        "title": "Fallback title",
        "content": "# Fallback heading\n\nREQUIRED-FALLBACK: an explicit managed fallback completed the request."
      }
    }"##;
    let (endpoint, _captured) = serve_json_once(response);
    let target = Url::parse(&endpoint.as_str().replace("/v2/scrape", "/target")).unwrap();
    let config = JinaConfig {
        endpoint,
        ..JinaConfig::default()
    };

    let mut request = ReadRequest::url(target);
    request.url_policy.allow_private_networks = true;
    request.url_policy.budget.max_origin_requests = 0;
    request
        .url_policy
        .fallbacks
        .push(Acquisition::Managed(ManagedProvider::Jina(config)));

    let execution = Reader::new().execute(request).await;
    let article = match &execution.outcome {
        ExecutionOutcome::Success(article) => article,
        ExecutionOutcome::Failure(error) => panic!("fallback failed: {error}"),
    };
    assert!(article.content.contains("REQUIRED-FALLBACK"));
    assert!(article.provenance.degraded);
    assert!(
        article
            .warnings
            .iter()
            .any(|warning| warning.code == "acquisition_fallback")
    );
    assert!(
        execution
            .attempts
            .iter()
            .any(|attempt| attempt.engine == "acquire:origin" && attempt.failure.is_some())
    );
    assert!(
        execution
            .attempts
            .iter()
            .any(|attempt| attempt.engine == "acquire:jina" && attempt.selected)
    );
}

#[tokio::test]
async fn yxt_adapter_submits_polls_and_fetches_artifact() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut requests = Vec::new();
        for index in 0..4 {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            requests.push(request);
            match index {
                0 => send_json(&mut stream, r#"{"status":20000,"tid":"task-42"}"#),
                1 => send_json(&mut stream, r#"{"task_status":"finished"}"#),
                2 => send_json(
                    &mut stream,
                    &format!(r#"{{"url":"http://{address}/artifact"}}"#),
                ),
                3 => send_json(
                    &mut stream,
                    r##"{"data":{"content_blocks":[{"text":"# YXT result"},{"text":"REQUIRED-YXT: task artifacts become sanitized HTML."}]}}"##,
                ),
                _ => unreachable!(),
            }
        }
        sender.send(requests).unwrap();
    });

    let base = Url::parse(&format!("http://{address}/")).unwrap();
    let mut request = ReadRequest::url(base.join("target").unwrap());
    request.url_policy.allow_private_networks = true;
    request.url_policy.acquisition = Acquisition::Managed(ManagedProvider::Yxt(YxtConfig {
        endpoint: base,
        authorization: SecretString::from("OK"),
        client: "AI_DIGGER".to_string(),
        poll_interval: std::time::Duration::from_millis(1),
    }));
    let execution = Reader::new().execute(request).await;
    let article = match execution.outcome {
        ExecutionOutcome::Success(article) => article,
        ExecutionOutcome::Failure(error) => panic!("YXT failed: {error}"),
    };
    assert!(article.content.contains("REQUIRED-YXT"));
    assert!(article.provenance.markdown_native);
    let requests = receiver.recv().unwrap();
    assert!(requests[0].starts_with("POST /document/parse/async"));
    assert!(requests[1].starts_with("POST /task/query/status"));
    assert!(requests[2].starts_with("POST /task/query/result"));
    assert!(requests[3].starts_with("GET /artifact"));
}
