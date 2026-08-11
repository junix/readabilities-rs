use std::time::Instant;

use unicode_segmentation::UnicodeSegmentation;
use url::Url;

use crate::error::{ErrorKind, ReadError, Result};
use crate::model::{
    Article, AttemptRecord, Backend, ExtractionMode, ExtractionOptions, Metadata, Provenance,
    QualityBand, QualitySignals, RemovalRecord, SCHEMA_VERSION, SnapshotObservations, Stage,
    StageRecord, Warning,
};
use crate::sanitize;
use crate::site::{self, SiteConfig};

pub(crate) struct LocalExtraction {
    pub article: Article,
    pub attempts: Vec<AttemptRecord>,
    pub removals: Vec<RemovalRecord>,
    pub stages: Vec<StageRecord>,
}

struct Candidate {
    html: String,
    metadata: Metadata,
    signals: QualitySignals,
    engine: String,
    site_extractor: Option<String>,
    removals: Vec<RemovalRecord>,
    elapsed_ms: u64,
    mode: ExtractionMode,
}

pub(crate) fn extract(
    html: &str,
    base_url: Option<&Url>,
    options: &ExtractionOptions,
    backend: Backend,
    snapshot: SnapshotObservations,
    markdown_native: bool,
    site_configs: &[SiteConfig],
) -> Result<LocalExtraction> {
    let validate_started = Instant::now();
    if html.trim().is_empty() {
        return Err(ReadError::new(
            ErrorKind::InvalidInput,
            Stage::Validate,
            backend,
            "HTML input is empty",
        ));
    }
    let mut stages = vec![StageRecord {
        stage: Stage::Validate,
        backend,
        elapsed_ms: elapsed_ms(validate_started),
        detail: format!("validated {} input bytes", html.len()),
    }];

    let matched_site = site::find(site_configs, base_url);
    let prepared_html = if let Some(config) = matched_site {
        site::remove_configured_noise(html, config).map_err(|error| {
            ReadError::new(
                ErrorKind::InvalidInput,
                Stage::Validate,
                backend,
                error.to_string(),
            )
        })?
    } else {
        html.to_string()
    };
    let content_selector = matched_site.and_then(|config| config.content_selector.as_deref());

    let primary_mode = match options.mode {
        ExtractionMode::Ensemble => ExtractionMode::Balanced,
        mode => mode,
    };
    let mut candidates = Vec::new();
    let primary = run_decruft(
        &prepared_html,
        base_url,
        options,
        primary_mode,
        content_selector,
    );
    let primary_is_weak =
        primary.signals.text_chars < 240 || matches!(primary.signals_band(), QualityBand::Weak);
    candidates.push(primary);

    if primary_is_weak && primary_mode != ExtractionMode::Conservative {
        candidates.push(run_decruft(
            &prepared_html,
            base_url,
            options,
            ExtractionMode::Conservative,
            content_selector,
        ));
    }

    if options.mode == ExtractionMode::Ensemble {
        #[cfg(feature = "ensemble")]
        candidates.push(run_trafilatura(&prepared_html, base_url, options));

        #[cfg(not(feature = "ensemble"))]
        return Err(crate::model::unsupported_feature(
            "ensemble",
            Backend::Local,
        ));
    }

    let selected_index = select_candidate(&candidates);
    let attempts = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| AttemptRecord {
            engine: candidate.engine.clone(),
            mode: candidate.mode,
            elapsed_ms: candidate.elapsed_ms,
            selected: index == selected_index,
            quality: Some(candidate.signals_band()),
            score: Some(candidate.signals.score),
            failure: None,
        })
        .collect::<Vec<_>>();
    let selected = candidates.swap_remove(selected_index);
    let selected_via_fallback =
        selected.engine != "decruft" || selected.mode != primary_mode || selected_index != 0;

    let text = decruft::strip_html_tags(&selected.html);
    let text_chars = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .count();
    if text_chars < options.minimum_text_chars {
        return Err(ReadError::new(
            ErrorKind::NoContent,
            Stage::Locate,
            backend,
            "no readable content remained after extraction and sanitization",
        )
        .with_attempts(attempts));
    }

    let security_violations = sanitize::violations(&selected.html);
    if !security_violations.is_empty() {
        return Err(ReadError::new(
            ErrorKind::InternalInvariant,
            Stage::Sanitize,
            backend,
            format!(
                "final sanitizer invariant failed: {}",
                security_violations.join(", ")
            ),
        )
        .with_attempts(attempts));
    }

    stages.push(StageRecord {
        stage: Stage::Parse,
        backend,
        elapsed_ms: selected.elapsed_ms,
        detail: format!(
            "{} performed locate, clean, normalize, and site extraction",
            selected.engine
        ),
    });
    stages.push(StageRecord {
        stage: Stage::Sanitize,
        backend,
        elapsed_ms: 0,
        detail: "mandatory final active-content sanitizer passed".to_string(),
    });

    let mut warnings = Vec::new();
    let quality = selected.signals_band();
    if selected_via_fallback {
        warnings.push(Warning::new(
            "degraded_fallback",
            format!(
                "selected {} {:?} after the primary attempt was weaker",
                selected.engine, selected.mode
            ),
        ));
    }
    if quality == QualityBand::Weak {
        warnings.push(Warning::new(
            "weak_content",
            "the sanitized result is non-empty but has weak content signals",
        ));
    }

    let article = Article {
        schema_version: SCHEMA_VERSION,
        content: selected.html,
        metadata: selected.metadata,
        word_count: selected.signals.words,
        quality,
        signals: selected.signals,
        provenance: Provenance {
            backend,
            engine: selected.engine,
            site_extractor: selected.site_extractor,
            site_config: matched_site.map(|config| config.id.clone()),
            source_url: base_url.map(ToString::to_string),
            snapshot,
            degraded: selected_via_fallback,
            markdown_native,
        },
        warnings,
    };

    Ok(LocalExtraction {
        article,
        attempts,
        removals: selected.removals,
        stages,
    })
}

impl Candidate {
    fn signals_band(&self) -> QualityBand {
        quality_band(&self.signals)
    }
}

fn run_decruft(
    html: &str,
    base_url: Option<&Url>,
    options: &ExtractionOptions,
    mode: ExtractionMode,
    content_selector: Option<&str>,
) -> Candidate {
    let started = Instant::now();
    let mut decruft_options = decruft::DecruftOptions::default();
    decruft_options.url = base_url.map(ToString::to_string);
    decruft_options.debug = options.diagnostics;
    decruft_options.remove_images = !options.include_images;
    decruft_options.include_replies = options.include_replies;
    decruft_options.allow_network = false;
    decruft_options.content_selector = content_selector.map(ToOwned::to_owned);

    match mode {
        ExtractionMode::Balanced | ExtractionMode::Ensemble => {}
        ExtractionMode::Conservative => {
            decruft_options.remove_low_scoring = false;
            decruft_options.remove_partial_selectors = false;
            decruft_options.remove_content_patterns = false;
        }
        ExtractionMode::Aggressive => {
            decruft_options.include_replies = false;
            decruft_options.remove_small_images = true;
        }
    }

    let result = decruft::parse(html, &decruft_options);
    let sanitized = sanitize::clean(&result.content);
    let removals = result
        .debug
        .as_ref()
        .map(|debug| {
            debug
                .removals
                .iter()
                .take(200)
                .map(|removal| RemovalRecord {
                    step: removal.step.clone(),
                    selector: removal.selector.clone(),
                    reason: removal.reason.clone(),
                    preview: bounded_preview(&removal.text, 200),
                })
                .collect()
        })
        .unwrap_or_default();

    Candidate {
        signals: analyze(&sanitized),
        html: sanitized,
        metadata: Metadata {
            title: result.title,
            author: result.author,
            description: result.description,
            published: result.published,
            modified: result.modified,
            site: result.site,
            language: result.language,
            image: result.image,
            canonical_url: result.canonical_url,
            keywords: result.keywords,
        },
        engine: "decruft".to_string(),
        site_extractor: result.extractor_type,
        removals,
        elapsed_ms: elapsed_ms(started),
        mode,
    }
}

#[cfg(feature = "ensemble")]
fn run_trafilatura(html: &str, base_url: Option<&Url>, options: &ExtractionOptions) -> Candidate {
    let started = Instant::now();
    let mut trafilatura_options = trafilatura::Options::default()
        .with_fallback(true)
        .with_links(true)
        .with_images(options.include_images)
        .with_exclude_comments(!options.include_replies);
    if let Some(url) = base_url {
        trafilatura_options = trafilatura_options.with_url(url.clone());
    }
    let result = trafilatura::extract(html, &trafilatura_options);
    match result {
        Ok(result) => {
            let sanitized = sanitize::clean(&result.content_html);
            Candidate {
                signals: analyze(&sanitized),
                html: sanitized,
                metadata: Metadata {
                    title: non_empty(result.metadata.title),
                    author: non_empty(result.metadata.author),
                    description: non_empty(result.metadata.description),
                    published: result.metadata.date.map(|date| date.to_string()),
                    modified: None,
                    site: non_empty(result.metadata.sitename),
                    language: non_empty(result.metadata.language),
                    image: non_empty(result.metadata.image),
                    canonical_url: non_empty(result.metadata.url),
                    keywords: result.metadata.tags,
                },
                engine: "trafilatura-rs".to_string(),
                site_extractor: None,
                removals: Vec::new(),
                elapsed_ms: elapsed_ms(started),
                mode: ExtractionMode::Ensemble,
            }
        }
        Err(error) => Candidate {
            html: String::new(),
            metadata: Metadata::default(),
            signals: analyze(""),
            engine: "trafilatura-rs".to_string(),
            site_extractor: None,
            removals: Vec::new(),
            elapsed_ms: elapsed_ms(started),
            mode: ExtractionMode::Ensemble,
        }
        .with_failure_penalty(error.to_string()),
    }
}

#[cfg(feature = "ensemble")]
impl Candidate {
    fn with_failure_penalty(mut self, _message: String) -> Self {
        self.signals.score = i32::MIN / 2;
        self
    }
}

fn select_candidate(candidates: &[Candidate]) -> usize {
    let primary = &candidates[0];
    let primary_band = primary.signals_band();
    if primary_band != QualityBand::Weak {
        return 0;
    }

    candidates
        .iter()
        .enumerate()
        .max_by_key(|(_, candidate)| candidate.signals.score)
        .map_or(0, |(index, _)| index)
}

pub(crate) fn analyze(html: &str) -> QualitySignals {
    let text = decruft::strip_html_tags(html);
    let words = count_words(&text);
    let text_chars = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .count();
    let lowercase_html = html.to_ascii_lowercase();
    let lowercase_text = text.to_ascii_lowercase();
    let paragraphs = lowercase_html.matches("<p").count();
    let headings: usize = (1..=6)
        .map(|level| lowercase_html.matches(&format!("<h{level}")).count())
        .sum();
    let links = lowercase_html.matches("<a ").count();
    let code_blocks = lowercase_html.matches("<pre").count();
    let tables = lowercase_html.matches("<table").count();

    let length_score = i32::try_from(text_chars.min(1_200) / 6).unwrap_or(200);
    let structure_score = i32::try_from(
        paragraphs.min(12) * 5 + headings.min(8) * 4 + code_blocks.min(4) * 5 + tables.min(3) * 5,
    )
    .unwrap_or(0);
    let link_penalty = if text_chars == 0 {
        0
    } else {
        i32::try_from((links * 120).saturating_sub(text_chars) / 20).unwrap_or(0)
    };
    let noise_phrases = [
        "cookie policy",
        "sign up for our newsletter",
        "advertisement",
        "related articles",
        "all rights reserved",
        "privacy policy",
    ]
    .iter()
    .filter(|phrase| lowercase_text.contains(**phrase))
    .count();
    let noise_penalty = i32::try_from(noise_phrases).unwrap_or(i32::MAX / 18) * 18;

    QualitySignals {
        words,
        text_chars,
        paragraphs,
        headings,
        links,
        code_blocks,
        tables,
        score: length_score + structure_score - link_penalty - noise_penalty,
    }
}

fn quality_band(signals: &QualitySignals) -> QualityBand {
    if signals.text_chars >= 400 && (signals.words >= 55 || signals.paragraphs >= 3) {
        QualityBand::Strong
    } else if signals.text_chars >= 80 || signals.words >= 15 {
        QualityBand::Usable
    } else {
        QualityBand::Weak
    }
}

fn count_words(text: &str) -> usize {
    let unicode_words = text.unicode_words().count();
    let cjk_chars = text
        .chars()
        .filter(|character| {
            matches!(
                *character as u32,
                0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
            )
        })
        .count();
    unicode_words.max(cjk_chars)
}

#[cfg(feature = "ensemble")]
fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn bounded_preview(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOISY: &str = r#"<!doctype html><html><head><title>Useful Story</title></head><body>
        <header>Global brand</header><nav>Home Products Pricing</nav>
        <main><article><h1>Useful Story</h1>
        <p>The first useful paragraph explains the central idea in enough detail to be retained.</p>
        <p>The second useful paragraph adds evidence, examples, and a practical conclusion.</p>
        <pre><code class="language-rust">fn main() { println!("safe"); }</code></pre>
        </article><aside>Related articles and promotions</aside></main>
        <div class="cookie-banner">Accept cookies</div><footer>Privacy Policy Copyright</footer>
        <script>alert('bad')</script></body></html>"#;

    #[test]
    fn local_extraction_keeps_content_and_removes_chrome() {
        let result = extract(
            NOISY,
            Some(&Url::parse("https://example.test/story").unwrap()),
            &ExtractionOptions::default(),
            Backend::Local,
            SnapshotObservations::static_html(crate::SnapshotKind::CallerHtml),
            false,
            &[],
        )
        .unwrap();
        assert!(result.article.content.contains("first useful paragraph"));
        assert!(!result.article.content.contains("Global brand"));
        assert!(!result.article.content.contains("Accept cookies"));
        assert!(result.article.content.contains("<pre"));
        assert!(result.article.content.contains("fn main"));
        assert!(sanitize::violations(&result.article.content).is_empty());
    }

    #[test]
    fn quality_counts_cjk_content() {
        let signals =
            analyze("<p>这是一个足够清晰的中文正文段落，用于验证中文字符不会被计算为零。</p>");
        assert!(signals.words > 10);
    }
}
