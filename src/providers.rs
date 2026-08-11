use std::fmt::Write;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use pulldown_cmark::{Options as MarkdownOptions, Parser, html};
use reqwest::Client;
use secrecy::ExposeSecret;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

use crate::error::{ErrorKind, ReadError, Result, RetryAdvice};
use crate::model::{
    Backend, CostLedger, CostValue, FirecrawlConfig, JinaConfig, ManagedProvider, Metadata,
    RequestBudget, SnapshotKind, SnapshotObservations, Stage, StageRecord, YxtConfig,
};

pub(crate) struct ManagedPage {
    pub html: String,
    pub backend: Backend,
    pub snapshot: SnapshotObservations,
    pub markdown_native: bool,
    pub metadata: Metadata,
    pub stage: StageRecord,
}

pub(crate) async fn read(
    target: &Url,
    provider: ManagedProvider,
    budget: &RequestBudget,
    cost: &mut CostLedger,
) -> Result<ManagedPage> {
    if budget.max_billable_submissions == 0 {
        return Err(ReadError::new(
            ErrorKind::BudgetExceeded,
            Stage::RemoteSubmit,
            provider_backend(&provider),
            "managed provider denied because billable submission budget is zero",
        )
        .with_retry(RetryAdvice::IncreaseBudget));
    }

    let backend = provider_backend(&provider);
    let started = Instant::now();
    let future = async {
        match provider {
            ManagedProvider::Jina(config) => read_jina(target, &config, budget, cost).await,
            ManagedProvider::Firecrawl(config) => {
                read_firecrawl(target, &config, budget, cost).await
            }
            ManagedProvider::Yxt(config) => read_yxt(target, &config, budget, cost).await,
        }
    };
    tokio::time::timeout(budget.deadline, future)
        .await
        .map_err(|_| {
            ReadError::new(
                ErrorKind::Timeout,
                Stage::RemotePoll,
                backend,
                format!(
                    "managed reader exceeded {} ms deadline",
                    budget.deadline.as_millis()
                ),
            )
            .with_retry(RetryAdvice::IncreaseBudget)
        })?
        .map(|mut page| {
            page.stage.elapsed_ms =
                u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            page
        })
}

async fn read_jina(
    target: &Url,
    config: &JinaConfig,
    budget: &RequestBudget,
    cost: &mut CostLedger,
) -> Result<ManagedPage> {
    reserve_submission(Backend::Jina, budget, cost)?;
    let endpoint = Url::parse(&format!(
        "{}/{target}",
        config.endpoint.as_str().trim_end_matches('/')
    ))
    .map_err(|error| provider_error(Backend::Jina, Stage::RemoteSubmit, error.to_string()))?;
    let client = provider_client(Backend::Jina)?;
    let mut request = client
        .get(endpoint)
        .header(reqwest::header::ACCEPT, "application/json")
        .header("x-respond-with", "markdown");
    if config.no_cache {
        request = request.header("x-no-cache", "true");
    }
    if let Some(api_key) = &config.api_key {
        request = request.bearer_auth(api_key.expose_secret());
    }
    reserve_remote_request(Backend::Jina, budget, cost)?;
    let response = request
        .send()
        .await
        .map_err(|error| request_error(Backend::Jina, &error))?;
    let bytes = bounded_body(response, Backend::Jina, budget, cost).await?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        provider_error(
            Backend::Jina,
            Stage::RemoteSubmit,
            format!("Jina JSON response was invalid: {error}"),
        )
    })?;
    let data = value.get("data").unwrap_or(&value);
    let content = data.get("content").and_then(Value::as_str).ok_or_else(|| {
        provider_error(
            Backend::Jina,
            Stage::RemoteSubmit,
            "Jina response omitted data.content",
        )
    })?;
    if content.trim().is_empty() {
        return Err(provider_error(
            Backend::Jina,
            Stage::RemoteSubmit,
            "Jina returned empty content",
        ));
    }
    Ok(ManagedPage {
        html: markdown_to_html(content),
        backend: Backend::Jina,
        snapshot: SnapshotObservations::static_html(SnapshotKind::ManagedMarkdown),
        markdown_native: true,
        metadata: Metadata {
            title: string_field(data, "title"),
            description: string_field(data, "description"),
            canonical_url: string_field(data, "url"),
            ..Metadata::default()
        },
        stage: StageRecord {
            stage: Stage::RemoteSubmit,
            backend: Backend::Jina,
            elapsed_ms: 0,
            detail: "explicit Jina Reader request returned native Markdown".to_string(),
        },
    })
}

async fn read_firecrawl(
    target: &Url,
    config: &FirecrawlConfig,
    budget: &RequestBudget,
    cost: &mut CostLedger,
) -> Result<ManagedPage> {
    reserve_submission(Backend::Firecrawl, budget, cost)?;
    let client = provider_client(Backend::Firecrawl)?;
    let body = json!({
        "url": target,
        "formats": ["markdown", "html"],
        "onlyMainContent": true,
        "removeBase64Images": true,
        "blockAds": true,
        "skipTlsVerification": false,
        "storeInCache": config.store_in_cache,
        "zeroDataRetention": config.zero_data_retention,
        "timeout": u32::try_from(budget.deadline.as_millis()).unwrap_or(u32::MAX),
    });
    reserve_remote_request(Backend::Firecrawl, budget, cost)?;
    let response = client
        .post(config.endpoint.clone())
        .bearer_auth(config.api_key.expose_secret())
        .json(&body)
        .send()
        .await
        .map_err(|error| request_error(Backend::Firecrawl, &error))?;
    let bytes = bounded_body(response, Backend::Firecrawl, budget, cost).await?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        provider_error(
            Backend::Firecrawl,
            Stage::RemoteSubmit,
            format!("Firecrawl JSON response was invalid: {error}"),
        )
    })?;
    if !value
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let message = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("Firecrawl reported an unsuccessful scrape");
        return Err(provider_error(
            Backend::Firecrawl,
            Stage::RemoteSubmit,
            message,
        ));
    }
    let data = value.get("data").ok_or_else(|| {
        provider_error(
            Backend::Firecrawl,
            Stage::RemoteSubmit,
            "Firecrawl response omitted data",
        )
    })?;
    let html_content = data
        .get("html")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty());
    let markdown = data
        .get("markdown")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty());
    let (html_content, markdown_native, kind) = if let Some(content) = html_content {
        (content.to_string(), false, SnapshotKind::ManagedHtml)
    } else if let Some(content) = markdown {
        (
            markdown_to_html(content),
            true,
            SnapshotKind::ManagedMarkdown,
        )
    } else {
        return Err(provider_error(
            Backend::Firecrawl,
            Stage::RemoteSubmit,
            "Firecrawl response contained neither HTML nor Markdown",
        ));
    };
    let metadata = data.get("metadata").unwrap_or(&Value::Null);
    Ok(ManagedPage {
        html: html_content,
        backend: Backend::Firecrawl,
        snapshot: SnapshotObservations::static_html(kind),
        markdown_native,
        metadata: Metadata {
            title: string_field(metadata, "title"),
            description: string_field(metadata, "description"),
            language: string_field(metadata, "language"),
            canonical_url: string_field(metadata, "sourceURL"),
            ..Metadata::default()
        },
        stage: StageRecord {
            stage: Stage::RemoteSubmit,
            backend: Backend::Firecrawl,
            elapsed_ms: 0,
            detail: "explicit Firecrawl v2 scrape returned main-content output".to_string(),
        },
    })
}

async fn read_yxt(
    target: &Url,
    config: &YxtConfig,
    budget: &RequestBudget,
    cost: &mut CostLedger,
) -> Result<ManagedPage> {
    reserve_submission(Backend::Yxt, budget, cost)?;
    let client = provider_client(Backend::Yxt)?;
    let request_id = request_id(target);
    let submit_url = endpoint_join(&config.endpoint, "document/parse/async")
        .map_err(|error| provider_error(Backend::Yxt, Stage::RemoteSubmit, error.to_string()))?;
    let submit_body = json!({
        "rid": request_id,
        "client": config.client,
        "data_uri": target,
        "data_text": "",
        "doc_metadata": {},
        "parse_options": {
            "summary_options": {"enable_action": false},
            "abstract_options": {"enable_action": false},
            "keyword_options": {"enable_action": false}
        }
    });
    let submit = post_json(
        &client,
        submit_url,
        &config.authorization,
        &submit_body,
        Backend::Yxt,
        budget,
        cost,
    )
    .await?;
    if submit.get("status").and_then(Value::as_i64) != Some(20_000) {
        return Err(provider_error(
            Backend::Yxt,
            Stage::RemoteSubmit,
            format!(
                "YXT submission failed with status {:?}",
                submit.get("status")
            ),
        )
        .with_request_id(request_id));
    }
    let task_id = submit
        .get("tid")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            provider_error(
                Backend::Yxt,
                Stage::RemoteSubmit,
                "YXT submission omitted task ID",
            )
            .with_request_id(request_id.clone())
        })?
        .to_string();

    let status_url = endpoint_join(&config.endpoint, "task/query/status")
        .map_err(|error| provider_error(Backend::Yxt, Stage::RemotePoll, error.to_string()))?;
    loop {
        if cost.remote_requests >= budget.max_remote_requests {
            return Err(ReadError::new(
                ErrorKind::BudgetExceeded,
                Stage::RemotePoll,
                Backend::Yxt,
                "YXT polling exhausted the remote request budget",
            )
            .with_request_id(task_id)
            .with_retry(RetryAdvice::IncreaseBudget));
        }
        let status = post_json(
            &client,
            status_url.clone(),
            &config.authorization,
            &json!({"tid": task_id, "task_name": "document_parse"}),
            Backend::Yxt,
            budget,
            cost,
        )
        .await?;
        match status.get("task_status").and_then(Value::as_str) {
            Some("finished") => break,
            Some("failed" | "queue_failed") => {
                return Err(ReadError::new(
                    ErrorKind::RemoteJob,
                    Stage::RemotePoll,
                    Backend::Yxt,
                    "YXT task failed",
                )
                .with_request_id(task_id));
            }
            Some("pending" | "running") | None => {
                tokio::time::sleep(config.poll_interval).await;
            }
            Some(other) => {
                return Err(ReadError::new(
                    ErrorKind::RemoteJob,
                    Stage::RemotePoll,
                    Backend::Yxt,
                    format!("YXT returned unknown task state {other}"),
                )
                .with_request_id(task_id));
            }
        }
    }

    let result_url = endpoint_join(&config.endpoint, "task/query/result")
        .map_err(|error| provider_error(Backend::Yxt, Stage::RemotePoll, error.to_string()))?;
    let result = post_json(
        &client,
        result_url,
        &config.authorization,
        &json!({"tid": task_id, "task_name": "document_parse"}),
        Backend::Yxt,
        budget,
        cost,
    )
    .await?;
    let artifact_url = result.get("url").and_then(Value::as_str).ok_or_else(|| {
        provider_error(
            Backend::Yxt,
            Stage::RemotePoll,
            "YXT result omitted artifact URL",
        )
        .with_request_id(task_id.clone())
    })?;
    let artifact_url = Url::parse(artifact_url).map_err(|error| {
        provider_error(
            Backend::Yxt,
            Stage::RemotePoll,
            format!("YXT artifact URL was invalid: {error}"),
        )
        .with_request_id(task_id.clone())
    })?;
    reserve_remote_request(Backend::Yxt, budget, cost)?;
    let response =
        client.get(artifact_url).send().await.map_err(|error| {
            request_error(Backend::Yxt, &error).with_request_id(task_id.clone())
        })?;
    let bytes = bounded_body(response, Backend::Yxt, budget, cost).await?;
    let artifact: Value = serde_json::from_slice(&bytes).map_err(|error| {
        provider_error(
            Backend::Yxt,
            Stage::RemotePoll,
            format!("YXT artifact JSON was invalid: {error}"),
        )
        .with_request_id(task_id.clone())
    })?;
    let content = artifact
        .pointer("/data/content_blocks")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if content.trim().is_empty() {
        return Err(ReadError::new(
            ErrorKind::NoContent,
            Stage::RemotePoll,
            Backend::Yxt,
            "YXT artifact contained no content blocks",
        )
        .with_request_id(task_id));
    }
    Ok(ManagedPage {
        html: markdown_to_html(&content),
        backend: Backend::Yxt,
        snapshot: SnapshotObservations::static_html(SnapshotKind::ManagedMarkdown),
        markdown_native: true,
        metadata: Metadata::default(),
        stage: StageRecord {
            stage: Stage::RemotePoll,
            backend: Backend::Yxt,
            elapsed_ms: 0,
            detail: format!("explicit YXT job {task_id} returned native Markdown blocks"),
        },
    })
}

async fn post_json(
    client: &Client,
    url: Url,
    authorization: &secrecy::SecretString,
    body: &Value,
    backend: Backend,
    budget: &RequestBudget,
    cost: &mut CostLedger,
) -> Result<Value> {
    reserve_remote_request(backend, budget, cost)?;
    let response = client
        .post(url)
        .header(
            reqwest::header::AUTHORIZATION,
            authorization.expose_secret(),
        )
        .json(body)
        .send()
        .await
        .map_err(|error| request_error(backend, &error))?;
    let bytes = bounded_body(response, backend, budget, cost).await?;
    serde_json::from_slice(&bytes).map_err(|error| {
        provider_error(
            backend,
            Stage::RemotePoll,
            format!("provider JSON response was invalid: {error}"),
        )
    })
}

async fn bounded_body(
    response: reqwest::Response,
    backend: Backend,
    budget: &RequestBudget,
    cost: &mut CostLedger,
) -> Result<Vec<u8>> {
    let status = response.status();
    if !status.is_success() {
        let (kind, retry) = match status.as_u16() {
            401 | 403 => (ErrorKind::Authentication, RetryAdvice::Never),
            408 | 429 => (ErrorKind::RateLimit, RetryAdvice::RetryAfter),
            500..=599 => (ErrorKind::RemoteJob, RetryAdvice::RetrySameBackend),
            _ => (ErrorKind::RemoteJob, RetryAdvice::Never),
        };
        return Err(ReadError::new(
            kind,
            Stage::RemoteSubmit,
            backend,
            format!("provider returned HTTP {status}"),
        )
        .with_retry(retry));
    }
    if let Some(length) = response.content_length() {
        if usize::try_from(length).unwrap_or(usize::MAX) > budget.max_download_bytes {
            return Err(ReadError::new(
                ErrorKind::BudgetExceeded,
                Stage::RemoteSubmit,
                backend,
                "provider Content-Length exceeds byte budget",
            )
            .with_retry(RetryAdvice::IncreaseBudget));
        }
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| request_error(backend, &error))?;
        if bytes.len().saturating_add(chunk.len()) > budget.max_download_bytes {
            return Err(ReadError::new(
                ErrorKind::BudgetExceeded,
                Stage::RemoteSubmit,
                backend,
                "provider response exceeded byte budget",
            )
            .with_retry(RetryAdvice::IncreaseBudget));
        }
        bytes.extend_from_slice(&chunk);
    }
    cost.downloaded_bytes = cost.downloaded_bytes.saturating_add(bytes.len());
    Ok(bytes)
}

fn reserve_submission(
    backend: Backend,
    budget: &RequestBudget,
    cost: &mut CostLedger,
) -> Result<()> {
    if cost.billable_submissions >= budget.max_billable_submissions {
        return Err(ReadError::new(
            ErrorKind::BudgetExceeded,
            Stage::RemoteSubmit,
            backend,
            "billable submission budget exhausted",
        )
        .with_retry(RetryAdvice::IncreaseBudget));
    }
    cost.billable_submissions += 1;
    cost.monetary_cost = CostValue::Unknown;
    Ok(())
}

fn reserve_remote_request(
    backend: Backend,
    budget: &RequestBudget,
    cost: &mut CostLedger,
) -> Result<()> {
    if cost.remote_requests >= budget.max_remote_requests {
        return Err(ReadError::new(
            ErrorKind::BudgetExceeded,
            Stage::RemoteSubmit,
            backend,
            "remote request budget exhausted",
        )
        .with_retry(RetryAdvice::IncreaseBudget));
    }
    cost.remote_requests += 1;
    Ok(())
}

fn provider_client(backend: Backend) -> Result<Client> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(format!("readabilities-rs/{}", crate::VERSION))
        .build()
        .map_err(|error| provider_error(backend, Stage::RemoteSubmit, error.to_string()))
}

fn request_error(backend: Backend, error: &reqwest::Error) -> ReadError {
    ReadError::new(
        if error.is_timeout() {
            ErrorKind::Timeout
        } else {
            ErrorKind::RemoteJob
        },
        Stage::RemoteSubmit,
        backend,
        error.to_string(),
    )
    .with_retry(RetryAdvice::RetrySameBackend)
}

fn provider_error(backend: Backend, stage: Stage, message: impl Into<String>) -> ReadError {
    ReadError::new(ErrorKind::RemoteJob, stage, backend, message)
}

fn provider_backend(provider: &ManagedProvider) -> Backend {
    match provider {
        ManagedProvider::Jina(_) => Backend::Jina,
        ManagedProvider::Firecrawl(_) => Backend::Firecrawl,
        ManagedProvider::Yxt(_) => Backend::Yxt,
    }
}

fn markdown_to_html(markdown: &str) -> String {
    let options = MarkdownOptions::ENABLE_TABLES
        | MarkdownOptions::ENABLE_FOOTNOTES
        | MarkdownOptions::ENABLE_STRIKETHROUGH
        | MarkdownOptions::ENABLE_TASKLISTS;
    let parser = Parser::new_ext(markdown, options);
    let mut output = String::from("<article>");
    html::push_html(&mut output, parser);
    output.push_str("</article>");
    output
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn request_id(url: &Url) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(url.as_str().as_bytes());
    hasher.update(timestamp.to_le_bytes());
    hasher
        .finalize()
        .iter()
        .take(16)
        .fold(String::with_capacity(32), |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to a String cannot fail");
            output
        })
}

fn endpoint_join(base: &Url, path: &str) -> std::result::Result<Url, url::ParseError> {
    Url::parse(&format!(
        "{}/{}",
        base.as_str().trim_end_matches('/'),
        path.trim_start_matches('/')
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_conversion_preserves_structure() {
        let converted = markdown_to_html("# Title\n\n```rust\nfn main() {}\n```\n");
        assert!(converted.contains("<h1>Title</h1>"));
        assert!(converted.contains("language-rust"));
    }

    #[test]
    fn provider_debug_output_is_redacted() {
        let provider = ManagedProvider::Jina(JinaConfig::default());
        let debug = format!("{provider:?}");
        assert!(debug.contains("redacted"));
    }
}
