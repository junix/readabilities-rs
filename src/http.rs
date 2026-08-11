use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::time::Instant;

use futures_util::StreamExt;
use reqwest::redirect::Policy;
use reqwest::{Client, StatusCode};
use url::Url;

use crate::error::{ErrorKind, ReadError, Result, RetryAdvice};
use crate::model::{
    Backend, CostLedger, SnapshotKind, SnapshotObservations, Stage, StageRecord, UrlPolicy,
};

pub(crate) struct AcquiredPage {
    pub html: String,
    pub final_url: Url,
    pub snapshot: SnapshotObservations,
    pub stage: StageRecord,
}

pub(crate) async fn fetch_origin(
    url: &Url,
    policy: &UrlPolicy,
    cost: &mut CostLedger,
) -> Result<AcquiredPage> {
    let started = Instant::now();
    let future = fetch_loop(url, policy, cost);
    tokio::time::timeout(policy.budget.deadline, future)
        .await
        .map_err(|_| {
            ReadError::new(
                ErrorKind::Timeout,
                Stage::Acquire,
                Backend::Origin,
                format!(
                    "origin acquisition exceeded {} ms",
                    policy.budget.deadline.as_millis()
                ),
            )
            .with_retry(RetryAdvice::IncreaseBudget)
        })?
        .map(|(html, final_url, lossy_decode)| AcquiredPage {
            html,
            final_url,
            snapshot: SnapshotObservations::static_html(SnapshotKind::OriginResponse),
            stage: StageRecord {
                stage: Stage::Acquire,
                backend: Backend::Origin,
                elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                detail: if lossy_decode {
                    "origin HTML fetched within limits; invalid UTF-8 was lossily decoded"
                        .to_string()
                } else {
                    "origin HTML fetched within redirect, byte, and deadline limits".to_string()
                },
            },
        })
}

async fn fetch_loop(
    initial_url: &Url,
    policy: &UrlPolicy,
    cost: &mut CostLedger,
) -> Result<(String, Url, bool)> {
    let initial_origin = origin(initial_url);
    let mut current = initial_url.clone();
    let mut redirects = 0_u8;

    loop {
        if cost.origin_requests >= policy.budget.max_origin_requests {
            return Err(ReadError::new(
                ErrorKind::BudgetExceeded,
                Stage::Acquire,
                Backend::Origin,
                "origin request budget exhausted",
            )
            .with_retry(RetryAdvice::IncreaseBudget));
        }
        cost.origin_requests += 1;

        let client = origin_client(&current, policy.allow_private_networks).await?;
        let response = client.get(current.clone()).send().await.map_err(|error| {
            ReadError::new(
                if error.is_timeout() {
                    ErrorKind::Timeout
                } else {
                    ErrorKind::OriginHttp
                },
                Stage::Acquire,
                Backend::Origin,
                error.to_string(),
            )
            .with_retry(RetryAdvice::RetrySameBackend)
        })?;

        if response.status().is_redirection() {
            if redirects >= policy.budget.max_redirects {
                return Err(ReadError::new(
                    ErrorKind::BudgetExceeded,
                    Stage::Acquire,
                    Backend::Origin,
                    "redirect budget exhausted",
                )
                .with_retry(RetryAdvice::IncreaseBudget));
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    ReadError::new(
                        ErrorKind::OriginHttp,
                        Stage::Acquire,
                        Backend::Origin,
                        "redirect response omitted a valid Location header",
                    )
                })?;
            let next = current.join(location).map_err(|error| {
                ReadError::new(
                    ErrorKind::OriginHttp,
                    Stage::Acquire,
                    Backend::Origin,
                    format!("invalid redirect target: {error}"),
                )
            })?;
            validate_redirect(&next, &initial_origin, policy.allow_cross_origin_redirects)?;
            redirects += 1;
            current = next;
            continue;
        }

        map_status(response.status())?;
        if let Some(length) = response.content_length() {
            if usize::try_from(length).unwrap_or(usize::MAX) > policy.budget.max_download_bytes {
                return Err(ReadError::new(
                    ErrorKind::BudgetExceeded,
                    Stage::Acquire,
                    Backend::Origin,
                    format!(
                        "Content-Length {length} exceeds byte budget {}",
                        policy.budget.max_download_bytes
                    ),
                )
                .with_retry(RetryAdvice::IncreaseBudget));
            }
        }

        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                ReadError::new(
                    ErrorKind::OriginHttp,
                    Stage::Acquire,
                    Backend::Origin,
                    error.to_string(),
                )
                .with_retry(RetryAdvice::RetrySameBackend)
            })?;
            let next_len = bytes.len().saturating_add(chunk.len());
            if next_len > policy.budget.max_download_bytes {
                return Err(ReadError::new(
                    ErrorKind::BudgetExceeded,
                    Stage::Acquire,
                    Backend::Origin,
                    format!(
                        "download exceeded byte budget {}",
                        policy.budget.max_download_bytes
                    ),
                )
                .with_retry(RetryAdvice::IncreaseBudget));
            }
            bytes.extend_from_slice(&chunk);
        }
        cost.downloaded_bytes = cost.downloaded_bytes.saturating_add(bytes.len());
        let lossy = std::str::from_utf8(&bytes).is_err();
        return Ok((String::from_utf8_lossy(&bytes).into_owned(), current, lossy));
    }
}

pub(crate) async fn validate_network_target(
    url: &Url,
    allow_private: bool,
) -> Result<Vec<SocketAddr>> {
    let host = url.host_str().ok_or_else(|| {
        ReadError::new(
            ErrorKind::InvalidInput,
            Stage::Validate,
            Backend::Origin,
            "URL must include a host",
        )
    })?;
    let port = url.port_or_known_default().ok_or_else(|| {
        ReadError::new(
            ErrorKind::InvalidInput,
            Stage::Validate,
            Backend::Origin,
            "URL must include a known or explicit port",
        )
    })?;
    let addresses = if let Ok(ip) = IpAddr::from_str(host) {
        vec![SocketAddr::new(ip, port)]
    } else {
        tokio::net::lookup_host((host, port))
            .await
            .map_err(|error| {
                ReadError::new(
                    ErrorKind::OriginHttp,
                    Stage::Acquire,
                    Backend::Origin,
                    format!("DNS lookup failed: {error}"),
                )
                .with_retry(RetryAdvice::RetrySameBackend)
            })?
            .collect::<Vec<_>>()
    };
    if addresses.is_empty() {
        return Err(ReadError::new(
            ErrorKind::OriginHttp,
            Stage::Acquire,
            Backend::Origin,
            "DNS lookup returned no addresses",
        ));
    }
    if !allow_private && addresses.iter().any(|address| is_non_public(address.ip())) {
        return Err(ReadError::new(
            ErrorKind::InvalidInput,
            Stage::Validate,
            Backend::Origin,
            "private, loopback, link-local, multicast, and documentation networks are denied by default",
        ));
    }
    Ok(addresses)
}

async fn origin_client(url: &Url, allow_private: bool) -> Result<Client> {
    let host = url.host_str().ok_or_else(|| {
        ReadError::new(
            ErrorKind::InvalidInput,
            Stage::Validate,
            Backend::Origin,
            "URL must include a host",
        )
    })?;
    let addresses = validate_network_target(url, allow_private).await?;
    Client::builder()
        .redirect(Policy::none())
        .no_proxy()
        .resolve_to_addrs(host, &addresses)
        .user_agent(format!(
            "readabilities-rs/{} (+https://github.com/junix/readabilities-rs)",
            crate::VERSION
        ))
        .build()
        .map_err(|error| {
            ReadError::new(
                ErrorKind::OriginHttp,
                Stage::Acquire,
                Backend::Origin,
                error.to_string(),
            )
        })
}

fn is_non_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.octets()[0] == 0
        }
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (segments[0] & 0xfe00) == 0xfc00
                || (segments[0] & 0xffc0) == 0xfe80
                || (segments[0] == 0x2001 && segments[1] == 0x0db8)
        }
    }
}

fn validate_redirect(
    target: &Url,
    initial_origin: &(String, Option<String>, Option<u16>),
    allow_cross_origin: bool,
) -> Result<()> {
    if !matches!(target.scheme(), "http" | "https") {
        return Err(ReadError::new(
            ErrorKind::OriginHttp,
            Stage::Acquire,
            Backend::Origin,
            "redirect target must use http or https",
        ));
    }
    if !target.username().is_empty() || target.password().is_some() {
        return Err(ReadError::new(
            ErrorKind::OriginHttp,
            Stage::Acquire,
            Backend::Origin,
            "redirect target contains embedded credentials",
        ));
    }
    if !allow_cross_origin && &origin(target) != initial_origin {
        return Err(ReadError::new(
            ErrorKind::OriginHttp,
            Stage::Acquire,
            Backend::Origin,
            "cross-origin redirect denied by policy",
        )
        .with_retry(RetryAdvice::ChooseAnotherBackend));
    }
    Ok(())
}

fn origin(url: &Url) -> (String, Option<String>, Option<u16>) {
    (
        url.scheme().to_string(),
        url.host_str().map(str::to_ascii_lowercase),
        url.port_or_known_default(),
    )
}

fn map_status(status: StatusCode) -> Result<()> {
    if status.is_success() {
        return Ok(());
    }
    let (kind, retry) = match status.as_u16() {
        401 | 403 => (ErrorKind::Authentication, RetryAdvice::Never),
        408 | 429 => (ErrorKind::RateLimit, RetryAdvice::RetryAfter),
        500..=599 => (ErrorKind::OriginHttp, RetryAdvice::RetrySameBackend),
        _ => (ErrorKind::OriginHttp, RetryAdvice::Never),
    };
    Err(ReadError::new(
        kind,
        Stage::Acquire,
        Backend::Origin,
        format!("origin returned HTTP {status}"),
    )
    .with_retry(retry))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_origin_redirects_are_denied_by_default() {
        let start = Url::parse("https://example.test/a").unwrap();
        let target = Url::parse("https://other.test/b").unwrap();
        let error = validate_redirect(&target, &origin(&start), false).unwrap_err();
        assert_eq!(error.kind, ErrorKind::OriginHttp);
    }

    #[test]
    fn same_origin_relative_redirect_is_allowed() {
        let start = Url::parse("https://example.test/a").unwrap();
        let target = start.join("/b").unwrap();
        assert!(validate_redirect(&target, &origin(&start), false).is_ok());
    }
}
