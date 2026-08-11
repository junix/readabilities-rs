use url::Url;

use crate::engine;
use crate::error::{ErrorKind, ReadError, Result};
use crate::model::{
    Article, Backend, CostLedger, Execution, ExecutionOutcome, ExtractionOptions, ReadRequest,
    SCHEMA_VERSION, SnapshotKind, SnapshotObservations, Source, Stage, UrlPolicy,
};
use crate::site::{SiteConfig, SiteConfigError};

#[cfg(feature = "http")]
use crate::model::{Acquisition, AttemptRecord, Metadata, StageRecord, Warning};

#[cfg(feature = "http")]
struct AcquiredInput {
    html: String,
    final_url: Url,
    backend: Backend,
    snapshot: SnapshotObservations,
    markdown_native: bool,
    metadata: Metadata,
    stage: StageRecord,
}

#[derive(Debug, Clone)]
pub struct Reader {
    defaults: ExtractionOptions,
    site_configs: Vec<SiteConfig>,
}

impl Default for Reader {
    fn default() -> Self {
        Self {
            defaults: ExtractionOptions::default(),
            site_configs: crate::site::builtins(),
        }
    }
}

impl Reader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_options(options: ExtractionOptions) -> Self {
        Self {
            defaults: options,
            site_configs: crate::site::builtins(),
        }
    }

    pub fn with_options_and_site_configs(
        options: ExtractionOptions,
        mut site_configs: Vec<SiteConfig>,
    ) -> std::result::Result<Self, SiteConfigError> {
        for config in &site_configs {
            config.validate()?;
        }
        site_configs.extend(crate::site::builtins());
        Ok(Self {
            defaults: options,
            site_configs,
        })
    }

    pub fn without_site_configs(options: ExtractionOptions) -> Self {
        Self {
            defaults: options,
            site_configs: Vec::new(),
        }
    }

    pub fn extract_html(&self, html: &str, base: Option<&Url>) -> Result<Article> {
        engine::extract(
            html,
            base,
            &self.defaults,
            Backend::Local,
            SnapshotObservations::static_html(SnapshotKind::CallerHtml),
            false,
            &self.site_configs,
        )
        .map(|result| result.article)
    }

    pub async fn read_url(&self, url: &Url, policy: &UrlPolicy) -> Result<Article> {
        let mut request = ReadRequest::url(url.clone());
        request.extraction = self.defaults.clone();
        request.url_policy = policy.clone();
        self.execute(request).await.into_result()
    }

    pub async fn execute(&self, request: ReadRequest) -> Execution {
        #[cfg(feature = "http")]
        let mut cost = CostLedger::default();
        #[cfg(not(feature = "http"))]
        let cost = CostLedger::default();
        let result = request
            .url_policy
            .budget
            .validate()
            .and_then(|()| validate_source(&request.source));
        if let Err(error) = result {
            return failure(error, Vec::new(), Vec::new(), cost);
        }

        match request.source {
            Source::Html { html, base_url } => match engine::extract(
                &html,
                base_url.as_ref(),
                &request.extraction,
                Backend::Local,
                SnapshotObservations::static_html(SnapshotKind::CallerHtml),
                false,
                &self.site_configs,
            ) {
                Ok(local) => Execution {
                    schema_version: SCHEMA_VERSION,
                    outcome: ExecutionOutcome::Success(Box::new(local.article)),
                    stages: local.stages,
                    attempts: local.attempts,
                    removals: local.removals,
                    cost,
                },
                Err(error) => failure(
                    error.clone(),
                    error.completed_attempts.clone(),
                    Vec::new(),
                    cost,
                ),
            },
            Source::Url(url) => {
                #[cfg(feature = "http")]
                {
                    self.execute_url(url, request.extraction, request.url_policy, &mut cost)
                        .await
                }

                #[cfg(not(feature = "http"))]
                {
                    let _ = url;
                    let _ = request.extraction;
                    let _ = request.url_policy;
                    failure(
                        crate::model::unsupported_feature("http", Backend::Origin),
                        Vec::new(),
                        Vec::new(),
                        cost,
                    )
                }
            }
        }
    }

    #[cfg(feature = "http")]
    async fn execute_url(
        &self,
        url: Url,
        extraction: ExtractionOptions,
        policy: UrlPolicy,
        cost: &mut CostLedger,
    ) -> Execution {
        let execution_started = std::time::Instant::now();
        let mut acquisitions = Vec::with_capacity(policy.fallbacks.len() + 1);
        acquisitions.push(policy.acquisition.clone());
        acquisitions.extend(policy.fallbacks.clone());
        let mut attempts = Vec::new();
        let mut stages = Vec::new();
        let mut last_error = None;

        for (index, acquisition) in acquisitions.into_iter().enumerate() {
            let backend = acquisition_backend(&acquisition);
            let remaining = policy
                .budget
                .deadline
                .saturating_sub(execution_started.elapsed());
            if remaining.is_zero() {
                last_error = Some(
                    ReadError::new(
                        ErrorKind::Timeout,
                        Stage::Acquire,
                        backend,
                        "URL execution exhausted its overall deadline",
                    )
                    .with_retry(crate::RetryAdvice::IncreaseBudget),
                );
                break;
            }
            let mut attempt_policy = policy.clone();
            attempt_policy.budget.deadline = remaining;
            let started = std::time::Instant::now();
            let acquired =
                tokio::time::timeout(remaining, acquire(&url, acquisition, &attempt_policy, cost))
                    .await
                    .unwrap_or_else(|_| {
                        Err(ReadError::new(
                            ErrorKind::Timeout,
                            Stage::Acquire,
                            backend,
                            "acquisition attempt exhausted the remaining overall deadline",
                        )
                        .with_retry(crate::RetryAdvice::IncreaseBudget))
                    });
            let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let acquired = match acquired {
                Ok(acquired) => acquired,
                Err(error) => {
                    stages.push(StageRecord {
                        stage: Stage::Acquire,
                        backend,
                        elapsed_ms,
                        detail: format!("acquisition attempt failed: {}", error.message),
                    });
                    attempts.push(AttemptRecord {
                        engine: format!("acquire:{backend}"),
                        mode: extraction.mode,
                        elapsed_ms,
                        selected: false,
                        quality: None,
                        score: None,
                        failure: Some(error.message.clone()),
                    });
                    last_error = Some(error);
                    continue;
                }
            };

            let acquisition_attempt = AttemptRecord {
                engine: format!("acquire:{backend}"),
                mode: extraction.mode,
                elapsed_ms,
                selected: true,
                quality: None,
                score: None,
                failure: None,
            };
            let extracted = engine::extract(
                &acquired.html,
                Some(&acquired.final_url),
                &extraction,
                acquired.backend,
                acquired.snapshot,
                acquired.markdown_native,
                &self.site_configs,
            );
            match extracted {
                Ok(mut local) => {
                    if index > 0 {
                        local.article.provenance.degraded = true;
                        local.article.warnings.push(Warning::new(
                            "acquisition_fallback",
                            format!("selected explicit fallback acquisition {backend}"),
                        ));
                    }
                    merge_missing_metadata(&mut local.article.metadata, acquired.metadata);
                    attempts.push(acquisition_attempt);
                    attempts.append(&mut local.attempts);
                    stages.push(acquired.stage);
                    stages.append(&mut local.stages);
                    return Execution {
                        schema_version: SCHEMA_VERSION,
                        outcome: ExecutionOutcome::Success(Box::new(local.article)),
                        stages,
                        attempts,
                        removals: local.removals,
                        cost: cost.clone(),
                    };
                }
                Err(error) => {
                    stages.push(acquired.stage);
                    attempts.push(AttemptRecord {
                        failure: Some(error.message.clone()),
                        selected: false,
                        ..acquisition_attempt
                    });
                    attempts.extend(error.completed_attempts.clone());
                    last_error = Some(error);
                }
            }
        }

        let error = last_error.unwrap_or_else(|| {
            ReadError::new(
                ErrorKind::InternalInvariant,
                Stage::Acquire,
                Backend::Origin,
                "URL policy contained no acquisition attempts",
            )
        });
        failure(error, attempts, stages, cost.clone())
    }
}

#[cfg(feature = "http")]
fn merge_missing_metadata(target: &mut crate::Metadata, provider: crate::Metadata) {
    if target.title.is_none() {
        target.title = provider.title;
    }
    if target.author.is_none() {
        target.author = provider.author;
    }
    if target.description.is_none() {
        target.description = provider.description;
    }
    if target.published.is_none() {
        target.published = provider.published;
    }
    if target.modified.is_none() {
        target.modified = provider.modified;
    }
    if target.site.is_none() {
        target.site = provider.site;
    }
    if target.language.is_none() {
        target.language = provider.language;
    }
    if target.image.is_none() {
        target.image = provider.image;
    }
    if target.canonical_url.is_none() {
        target.canonical_url = provider.canonical_url;
    }
    if target.keywords.is_empty() {
        target.keywords = provider.keywords;
    }
}

#[cfg(feature = "http")]
async fn acquire(
    url: &Url,
    acquisition: Acquisition,
    policy: &UrlPolicy,
    cost: &mut CostLedger,
) -> Result<AcquiredInput> {
    match acquisition {
        Acquisition::Origin => {
            let acquired = crate::http::fetch_origin(url, policy, cost).await?;
            Ok(AcquiredInput {
                html: acquired.html,
                final_url: acquired.final_url,
                backend: Backend::Origin,
                snapshot: acquired.snapshot,
                markdown_native: false,
                metadata: Metadata::default(),
                stage: acquired.stage,
            })
        }
        #[cfg(feature = "browser")]
        Acquisition::Browser(browser_policy) => {
            let acquired = crate::browser::fetch(
                url,
                &browser_policy,
                &policy.budget,
                policy.allow_private_networks,
                cost,
            )
            .await?;
            Ok(AcquiredInput {
                html: acquired.html,
                final_url: acquired.final_url,
                backend: Backend::Browser,
                snapshot: acquired.snapshot,
                markdown_native: false,
                metadata: Metadata::default(),
                stage: acquired.stage,
            })
        }
        #[cfg(feature = "providers")]
        Acquisition::Managed(provider) => {
            crate::http::validate_network_target(url, policy.allow_private_networks).await?;
            let managed = crate::providers::read(url, provider, &policy.budget, cost).await?;
            Ok(AcquiredInput {
                html: managed.html,
                final_url: url.clone(),
                backend: managed.backend,
                snapshot: managed.snapshot,
                markdown_native: managed.markdown_native,
                metadata: managed.metadata,
                stage: managed.stage,
            })
        }
    }
}

#[cfg(feature = "http")]
fn acquisition_backend(acquisition: &Acquisition) -> Backend {
    match acquisition {
        Acquisition::Origin => Backend::Origin,
        #[cfg(feature = "browser")]
        Acquisition::Browser(_) => Backend::Browser,
        #[cfg(feature = "providers")]
        Acquisition::Managed(provider) => match provider {
            crate::ManagedProvider::Jina(_) => Backend::Jina,
            crate::ManagedProvider::Firecrawl(_) => Backend::Firecrawl,
            crate::ManagedProvider::Yxt(_) => Backend::Yxt,
        },
    }
}

fn validate_source(source: &Source) -> Result<()> {
    match source {
        Source::Html { html, .. } if html.trim().is_empty() => Err(ReadError::new(
            ErrorKind::InvalidInput,
            Stage::Validate,
            Backend::Local,
            "HTML input is empty",
        )),
        Source::Url(url) => {
            if !matches!(url.scheme(), "http" | "https") {
                return Err(ReadError::new(
                    ErrorKind::InvalidInput,
                    Stage::Validate,
                    Backend::Origin,
                    "URL acquisition only supports http and https",
                ));
            }
            if !url.username().is_empty() || url.password().is_some() {
                return Err(ReadError::new(
                    ErrorKind::InvalidInput,
                    Stage::Validate,
                    Backend::Origin,
                    "embedded URL credentials are forbidden",
                ));
            }
            if url.host_str().is_none() {
                return Err(ReadError::new(
                    ErrorKind::InvalidInput,
                    Stage::Validate,
                    Backend::Origin,
                    "URL must include a host",
                ));
            }
            Ok(())
        }
        Source::Html { .. } => Ok(()),
    }
}

fn failure(
    mut error: ReadError,
    attempts: Vec<crate::AttemptRecord>,
    stages: Vec<crate::StageRecord>,
    cost: CostLedger,
) -> Execution {
    if error.completed_attempts.is_empty() {
        error.completed_attempts.clone_from(&attempts);
    }
    Execution {
        schema_version: SCHEMA_VERSION,
        outcome: ExecutionOutcome::Failure(error),
        stages,
        attempts,
        removals: Vec::new(),
        cost,
    }
}

#[cfg(all(test, feature = "http"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn execute_preserves_no_content_as_failure() {
        let execution = Reader::new().execute(ReadRequest::html("   ", None)).await;
        assert!(matches!(
            execution.outcome,
            ExecutionOutcome::Failure(ReadError {
                kind: ErrorKind::InvalidInput,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn rejects_embedded_url_credentials_before_network() {
        let url = Url::parse("https://user:secret@example.test/article").unwrap();
        let execution = Reader::new().execute(ReadRequest::url(url)).await;
        assert!(matches!(
            execution.outcome,
            ExecutionOutcome::Failure(ReadError {
                kind: ErrorKind::InvalidInput,
                ..
            })
        ));
    }

    #[test]
    fn rejects_invalid_library_site_config_before_extraction() {
        let config = SiteConfig {
            id: "invalid".to_string(),
            hosts: vec!["example.test".to_string()],
            path_prefixes: Vec::new(),
            content_selector: Some("main[".to_string()),
            remove_selectors: Vec::new(),
        };

        let result =
            Reader::with_options_and_site_configs(ExtractionOptions::default(), vec![config]);
        assert!(result.is_err());
    }
}
