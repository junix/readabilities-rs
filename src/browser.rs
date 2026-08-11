use std::time::Instant;

use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::handler::viewport::Viewport;
use futures_util::StreamExt;
use url::Url;

use crate::error::{ErrorKind, ReadError, Result, RetryAdvice};
use crate::http::AcquiredPage;
use crate::model::{
    Backend, BrowserPolicy, CostLedger, RequestBudget, SnapshotKind, SnapshotObservations, Stage,
    StageRecord,
};

pub(crate) async fn fetch(
    url: &Url,
    policy: &BrowserPolicy,
    budget: &RequestBudget,
    allow_private_networks: bool,
    cost: &mut CostLedger,
) -> Result<AcquiredPage> {
    crate::http::validate_network_target(url, allow_private_networks).await?;
    if cost.browser_launches >= u32::from(budget.max_browser_launches) {
        return Err(ReadError::new(
            ErrorKind::BudgetExceeded,
            Stage::Acquire,
            Backend::Browser,
            "browser launch budget exhausted",
        )
        .with_retry(RetryAdvice::IncreaseBudget));
    }
    cost.browser_launches += 1;
    let started = Instant::now();

    let mut builder = BrowserConfig::builder()
        .new_headless_mode()
        .incognito()
        .respect_https_errors()
        .request_timeout(budget.deadline)
        .launch_timeout(budget.deadline.min(std::time::Duration::from_secs(20)))
        .window_size(policy.viewport_width, policy.viewport_height)
        .viewport(Viewport {
            width: policy.viewport_width,
            height: policy.viewport_height,
            device_scale_factor: Some(1.0),
            emulating_mobile: false,
            is_landscape: policy.viewport_width >= policy.viewport_height,
            has_touch: false,
        });
    if policy.disable_cache {
        builder = builder.disable_cache();
    }
    if policy.block_images {
        builder = builder.arg(("blink-settings", "imagesEnabled=false"));
    }
    if let Some(executable) = &policy.executable {
        builder = builder.chrome_executable(executable);
    }
    let config = builder.build().map_err(|error| {
        ReadError::new(ErrorKind::Browser, Stage::Acquire, Backend::Browser, error)
            .with_retry(RetryAdvice::ChooseAnotherBackend)
    })?;

    let (mut browser, mut handler) = Browser::launch(config).await.map_err(|error| {
        ReadError::new(
            ErrorKind::Browser,
            Stage::Acquire,
            Backend::Browser,
            error.to_string(),
        )
        .with_retry(RetryAdvice::ChooseAnotherBackend)
    })?;
    let handler_task = tokio::spawn(async move {
        while let Some(event) = handler.next().await {
            if event.is_err() {
                break;
            }
        }
    });

    let result = {
        let page_work = async {
            let page = browser
                .new_page("about:blank")
                .await
                .map_err(|error| browser_error(&error))?;
            page.goto(url.as_str())
                .await
                .map_err(|error| browser_error(&error))?;
            if !policy.wait_after_load.is_zero() {
                tokio::time::sleep(policy.wait_after_load).await;
            }
            let html = page
                .content()
                .await
                .map_err(|error| browser_error(&error))?;
            if html.len() > budget.max_download_bytes {
                return Err(ReadError::new(
                    ErrorKind::BudgetExceeded,
                    Stage::Acquire,
                    Backend::Browser,
                    format!(
                        "serialized browser DOM exceeded byte budget {}",
                        budget.max_download_bytes
                    ),
                )
                .with_retry(RetryAdvice::IncreaseBudget));
            }
            Ok(html)
        };
        let remaining = budget.deadline.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            Err(browser_timeout(budget))
        } else {
            tokio::time::timeout(remaining, page_work)
                .await
                .map_err(|_| browser_timeout(budget))?
        }
    };

    let cleanup_remaining = budget.deadline.saturating_sub(started.elapsed());
    if !cleanup_remaining.is_zero() {
        let _ = tokio::time::timeout(cleanup_remaining, browser.close()).await;
    }
    handler_task.abort();

    let html = result?;
    cost.downloaded_bytes = cost.downloaded_bytes.saturating_add(html.len());
    Ok(AcquiredPage {
        html,
        final_url: url.clone(),
        snapshot: SnapshotObservations {
            kind: SnapshotKind::BrowserDom,
            javascript_executed: true,
            computed_styles: false,
            element_geometry: false,
            shadow_dom_flattened: false,
        },
        stage: StageRecord {
            stage: Stage::Acquire,
            backend: Backend::Browser,
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            detail: format!(
                "serialized live DOM after JS; viewport={}x{}, block_images={}, computed styles/geometry/shadow DOM not captured",
                policy.viewport_width, policy.viewport_height, policy.block_images
            ),
        },
    })
}

fn browser_timeout(budget: &RequestBudget) -> ReadError {
    ReadError::new(
        ErrorKind::Timeout,
        Stage::Acquire,
        Backend::Browser,
        format!(
            "browser acquisition exceeded {} ms deadline",
            budget.deadline.as_millis()
        ),
    )
    .with_retry(RetryAdvice::IncreaseBudget)
}

fn browser_error(error: &chromiumoxide::error::CdpError) -> ReadError {
    ReadError::new(
        ErrorKind::Browser,
        Stage::Acquire,
        Backend::Browser,
        error.to_string(),
    )
    .with_retry(RetryAdvice::ChooseAnotherBackend)
}
