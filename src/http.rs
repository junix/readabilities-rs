use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant, SystemTime};

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
    /// The response body grew past the byte budget mid-stream and was cut to
    /// the cap under `UrlPolicy::truncate_overrun` (declared-length overruns
    /// reject before this can happen).
    pub truncated_by_bytes: bool,
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
        .map(|(html, final_url, encoding, decode_errors, truncated)| AcquiredPage {
            html,
            final_url,
            truncated_by_bytes: truncated,
            snapshot: SnapshotObservations::static_html(SnapshotKind::OriginResponse),
            stage: StageRecord {
                stage: Stage::Acquire,
                backend: Backend::Origin,
                elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                detail: if truncated {
                    format!(
                        "origin body exceeded the byte budget and was truncated to {} bytes",
                        policy.budget.max_download_bytes
                    )
                } else if decode_errors {
                    format!(
                        "origin HTML fetched within limits; invalid {encoding} sequences were replaced"
                    )
                } else if encoding.eq_ignore_ascii_case("utf-8") {
                    "origin HTML fetched within redirect, byte, and deadline limits".to_string()
                } else {
                    format!(
                        "origin HTML fetched within redirect, byte, and deadline limits; decoded as {encoding}"
                    )
                },
            },
        })
}

async fn fetch_loop(
    initial_url: &Url,
    policy: &UrlPolicy,
    cost: &mut CostLedger,
) -> Result<(String, Url, &'static str, bool, bool)> {
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

        map_status(response.status(), response.headers())?;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
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
        let mut truncated_by_bytes = false;
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
                if !policy.truncate_overrun {
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
                // Bounded usable prefix for an under-reporting server (dsh
                // readCapped): fill exactly to the cap, drop the rest, stop
                // reading. Only DROPPED bytes count as truncation — a chunk
                // that exactly fills the remaining capacity keeps all its
                // bytes and reads on to observe EOF, so an exactly-at-cap
                // body is never flagged.
                let remaining = policy.budget.max_download_bytes - bytes.len();
                bytes.extend_from_slice(&chunk[..remaining]);
                truncated_by_bytes = true;
                break;
            }
            bytes.extend_from_slice(&chunk);
        }
        cost.downloaded_bytes = cost.downloaded_bytes.saturating_add(bytes.len());
        let decoded = crate::charset::decode_html(content_type.as_deref(), &bytes);
        return Ok((
            decoded.html,
            current,
            decoded.encoding,
            decoded.had_errors,
            truncated_by_bytes,
        ));
    }
}

pub(crate) async fn validate_network_target(
    url: &Url,
    allow_private: bool,
) -> Result<Vec<SocketAddr>> {
    let host = url.host().ok_or_else(|| {
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
    let addresses = match host {
        url::Host::Ipv4(ip) => vec![SocketAddr::new(IpAddr::V4(ip), port)],
        url::Host::Ipv6(ip) => vec![SocketAddr::new(IpAddr::V6(ip), port)],
        url::Host::Domain(host) => tokio::net::lookup_host((host, port))
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
            .collect::<Vec<_>>(),
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
    let host = url.host().ok_or_else(|| {
        ReadError::new(
            ErrorKind::InvalidInput,
            Stage::Validate,
            Backend::Origin,
            "URL must include a host",
        )
    })?;
    let host = match host {
        url::Host::Domain(host) => host.to_string(),
        url::Host::Ipv4(ip) => ip.to_string(),
        url::Host::Ipv6(ip) => ip.to_string(),
    };
    let addresses = validate_network_target(url, allow_private).await?;
    Client::builder()
        .redirect(Policy::none())
        .no_proxy()
        .resolve_to_addrs(&host, &addresses)
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
    match canonicalize_ip(ip) {
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

/// Collapse IPv6 forms that route to an embedded IPv4 destination before the
/// address is classified. Otherwise IPv4-mapped loopback/private addresses and
/// the RFC 6052 well-known NAT64 prefix bypass the IPv4 deny rules.
fn canonicalize_ip(ip: IpAddr) -> IpAddr {
    let IpAddr::V6(ipv6) = ip else {
        return ip;
    };

    if let Some(ipv4) = ipv6.to_ipv4_mapped() {
        return IpAddr::V4(ipv4);
    }

    let segments = ipv6.segments();
    if segments[0] == 0x0064 && segments[1] == 0xff9b && segments[2..6] == [0, 0, 0, 0] {
        let octets = ipv6.octets();
        return IpAddr::V4(std::net::Ipv4Addr::new(
            octets[12], octets[13], octets[14], octets[15],
        ));
    }

    ip
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

fn map_status(status: StatusCode, headers: &reqwest::header::HeaderMap) -> Result<()> {
    if status.is_success() {
        return Ok(());
    }
    let (kind, retry) = match status.as_u16() {
        401 | 403 => (ErrorKind::Authentication, RetryAdvice::Never),
        408 | 429 => (ErrorKind::RateLimit, RetryAdvice::RetryAfter),
        500..=599 => (ErrorKind::OriginHttp, RetryAdvice::RetrySameBackend),
        _ => (ErrorKind::OriginHttp, RetryAdvice::Never),
    };
    let mut error = ReadError::new(
        kind,
        Stage::Acquire,
        Backend::Origin,
        format!("origin returned HTTP {status}"),
    )
    .with_retry(retry);
    // Timing is captured only while the origin leaves the retry decision
    // open; absent and malformed headers simply leave the field unset, and
    // extraction never sleeps on the advice.
    if retry != RetryAdvice::Never {
        let advised = headers
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| match parse_retry_after(value, SystemTime::now()) {
                ParsedRetryAfter::Delay(secs) => Some(secs),
                ParsedRetryAfter::Past => Some(0),
                ParsedRetryAfter::Malformed => None,
            });
        if let Some(secs) = advised {
            error = error.with_retry_after_secs(secs);
        }
    }
    Err(error)
}

/// A parsed `Retry-After` header value (RFC 9110 section 10.2.3). The three
/// cases callers must treat differently are kept distinct: a wait to honor,
/// a date whose wait has already elapsed, and a value that is no delay at
/// all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParsedRetryAfter {
    /// Delta-seconds, or the whole-second distance to a future HTTP date.
    Delay(u64),
    /// A syntactically valid HTTP date at or before the reference clock.
    Past,
    /// Neither a delta-seconds value nor a parseable HTTP date.
    Malformed,
}

/// Parses a `Retry-After` header value: either delta-seconds or an HTTP
/// date. `now` is the reference clock, injected so tests can pin it; the
/// acquisition path passes the wall clock and never sleeps on the result.
fn parse_retry_after(value: &str, now: SystemTime) -> ParsedRetryAfter {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        return value
            .parse::<u64>()
            .map_or(ParsedRetryAfter::Malformed, ParsedRetryAfter::Delay);
    }
    match parse_http_date(value).map(|at| at.duration_since(now)) {
        Some(Ok(delay)) => ParsedRetryAfter::Delay(delay.as_secs()),
        Some(Err(_)) => ParsedRetryAfter::Past,
        None => ParsedRetryAfter::Malformed,
    }
}

/// Parses the IMF-fixdate HTTP date format (`Sun, 06 Nov 1994 08:49:37 GMT`)
/// into an absolute time. The day name is redundant and ignored; the zone
/// must be GMT, the only zone RFC 9110 permits.
fn parse_http_date(value: &str) -> Option<SystemTime> {
    let mut fields = value.split_whitespace();
    let _day_name = fields.next()?;
    let day: u32 = fields.next()?.parse().ok()?;
    let month = month_index(fields.next()?)?;
    let year: i32 = fields.next()?.parse().ok()?;
    let clock = fields.next()?;
    let zone = fields.next()?;
    if fields.next().is_some() || !zone.eq_ignore_ascii_case("gmt") {
        return None;
    }
    if !(1..=31).contains(&day) || !(0..=9999).contains(&year) {
        return None;
    }
    let mut clock_fields = clock.split(':');
    let hour: u32 = clock_fields.next()?.parse().ok()?;
    let minute: u32 = clock_fields.next()?.parse().ok()?;
    let second: u32 = clock_fields.next()?.parse().ok()?;
    if clock_fields.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + i64::from(hour * 3_600 + minute * 60 + second);
    u64::try_from(secs)
        .ok()
        .map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
}

fn month_index(name: &str) -> Option<u32> {
    Some(match name {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    })
}

/// Days since the Unix epoch for a proleptic Gregorian calendar date
/// (Howard Hinnant's `days_from_civil` algorithm).
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = i64::from(if month <= 2 { year - 1 } else { year });
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let month = i64::from(month);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
