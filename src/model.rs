use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use url::Url;

#[cfg(any(not(feature = "http"), not(feature = "ensemble")))]
use crate::RetryAdvice;
use crate::{ErrorKind, ReadError, Result};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Local,
    Origin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Validate,
    Acquire,
    Parse,
    Locate,
    Clean,
    Normalize,
    Sanitize,
    Render,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionMode {
    #[default]
    Balanced,
    Conservative,
    Aggressive,
    Ensemble,
}

impl FromStr for ExtractionMode {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "balanced" => Ok(Self::Balanced),
            "conservative" => Ok(Self::Conservative),
            "aggressive" => Ok(Self::Aggressive),
            "ensemble" => Ok(Self::Ensemble),
            other => Err(format!("unsupported extraction mode: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityBand {
    Strong,
    Usable,
    Weak,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    Html,
    Markdown,
    Text,
    Json,
}

impl FromStr for OutputFormat {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "html" => Ok(Self::Html),
            "markdown" | "md" => Ok(Self::Markdown),
            "text" | "txt" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            other => Err(format!("unsupported output format: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Metadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warning {
    pub code: String,
    pub message: String,
}

impl Warning {
    pub(crate) fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotKind {
    CallerHtml,
    OriginResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotObservations {
    pub kind: SnapshotKind,
    pub javascript_executed: bool,
    pub computed_styles: bool,
    pub element_geometry: bool,
    pub shadow_dom_flattened: bool,
}

impl SnapshotObservations {
    pub(crate) fn static_html(kind: SnapshotKind) -> Self {
        Self {
            kind,
            javascript_executed: false,
            computed_styles: false,
            element_geometry: false,
            shadow_dom_flattened: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub backend: Backend,
    pub engine: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_extractor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_config: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    pub snapshot: SnapshotObservations,
    pub degraded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualitySignals {
    pub words: usize,
    pub text_chars: usize,
    pub paragraphs: usize,
    pub headings: usize,
    pub links: usize,
    pub code_blocks: usize,
    pub tables: usize,
    pub score: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Article {
    pub schema_version: u32,
    pub content: String,
    pub metadata: Metadata,
    pub word_count: usize,
    pub quality: QualityBand,
    pub signals: QualitySignals,
    pub provenance: Provenance,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
}

impl Article {
    pub fn render(&self, format: OutputFormat) -> Result<String> {
        match format {
            OutputFormat::Html => Ok(self.content.clone()),
            OutputFormat::Markdown => crate::markdown::render(&self.content).map_err(|error| {
                ReadError::new(
                    ErrorKind::Render,
                    Stage::Render,
                    self.provenance.backend,
                    error.to_string(),
                )
            }),
            OutputFormat::Text => Ok(crate::native::strip_html_tags(&self.content)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")),
            OutputFormat::Json => serde_json::to_string_pretty(self).map_err(|error| {
                ReadError::new(
                    ErrorKind::Render,
                    Stage::Render,
                    self.provenance.backend,
                    error.to_string(),
                )
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Source {
    Html { html: String, base_url: Option<Url> },
    Url(Url),
}

#[derive(Debug, Clone)]
pub struct ExtractionOptions {
    pub mode: ExtractionMode,
    pub include_images: bool,
    pub include_replies: bool,
    pub minimum_text_chars: usize,
    /// Collect bounded removal evidence. Disabled by default so ordinary
    /// extraction does not pay the extractor's diagnostic bookkeeping cost.
    pub diagnostics: bool,
}

impl Default for ExtractionOptions {
    fn default() -> Self {
        Self {
            mode: ExtractionMode::Balanced,
            include_images: true,
            include_replies: true,
            minimum_text_chars: 2,
            diagnostics: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RequestBudget {
    pub deadline: Duration,
    pub max_origin_requests: u32,
    pub max_download_bytes: usize,
    pub max_redirects: u8,
}

impl Default for RequestBudget {
    fn default() -> Self {
        Self {
            deadline: Duration::from_secs(30),
            max_origin_requests: 5,
            max_download_bytes: 8 * 1024 * 1024,
            max_redirects: 4,
        }
    }
}

impl RequestBudget {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.deadline.is_zero() || self.max_download_bytes == 0 {
            return Err(ReadError::new(
                ErrorKind::InvalidInput,
                Stage::Validate,
                Backend::Local,
                "deadline and max_download_bytes must be positive",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum Acquisition {
    Origin,
}

#[derive(Debug, Clone)]
pub struct UrlPolicy {
    pub acquisition: Acquisition,
    /// Ordered, explicitly authorized fallbacks. No fallback is inferred.
    pub fallbacks: Vec<Acquisition>,
    pub budget: RequestBudget,
    pub allow_cross_origin_redirects: bool,
    pub allow_private_networks: bool,
}

impl Default for UrlPolicy {
    fn default() -> Self {
        Self {
            acquisition: Acquisition::Origin,
            fallbacks: Vec::new(),
            budget: RequestBudget::default(),
            allow_cross_origin_redirects: false,
            allow_private_networks: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReadRequest {
    pub source: Source,
    pub extraction: ExtractionOptions,
    pub url_policy: UrlPolicy,
}

impl ReadRequest {
    pub fn html(html: impl Into<String>, base_url: Option<Url>) -> Self {
        Self {
            source: Source::Html {
                html: html.into(),
                base_url,
            },
            extraction: ExtractionOptions::default(),
            url_policy: UrlPolicy::default(),
        }
    }

    pub fn url(url: Url) -> Self {
        Self {
            source: Source::Url(url),
            extraction: ExtractionOptions::default(),
            url_policy: UrlPolicy::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageRecord {
    pub stage: Stage,
    pub backend: Backend,
    pub elapsed_ms: u64,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub engine: String,
    pub mode: ExtractionMode,
    pub elapsed_ms: u64,
    pub selected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<QualityBand>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemovalRecord {
    pub step: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CostLedger {
    pub origin_requests: u32,
    pub downloaded_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", content = "result", rename_all = "snake_case")]
pub enum ExecutionOutcome {
    Success(Box<Article>),
    Failure(ReadError),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Execution {
    pub schema_version: u32,
    pub outcome: ExecutionOutcome,
    pub stages: Vec<StageRecord>,
    pub attempts: Vec<AttemptRecord>,
    pub removals: Vec<RemovalRecord>,
    pub cost: CostLedger,
}

impl Execution {
    pub fn into_result(self) -> Result<Article> {
        match self.outcome {
            ExecutionOutcome::Success(article) => Ok(*article),
            ExecutionOutcome::Failure(error) => Err(error),
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}",
            serde_json::to_value(self)
                .unwrap_or_default()
                .as_str()
                .unwrap_or("unknown")
        )
    }
}

#[cfg(any(not(feature = "http"), not(feature = "ensemble")))]
pub(crate) fn unsupported_feature(feature: &str, backend: Backend) -> ReadError {
    ReadError::new(
        ErrorKind::Unsupported,
        Stage::Validate,
        backend,
        format!("this build does not include the `{feature}` Cargo feature"),
    )
    .with_retry(RetryAdvice::ChooseAnotherBackend)
}
