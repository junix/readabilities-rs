use std::time::Instant;

use unicode_segmentation::UnicodeSegmentation;
use url::Url;

use crate::depth;
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
    // Depth screen before any parser sees the bytes: tree building cost
    // grows superlinearly with nesting, so a depth bomb fails fast here
    // instead of burning the budget in the DOM builder (dsh degrades to raw
    // HTML; this pipeline reports a structured error instead).
    if depth::exceeds_depth(html) {
        return Err(ReadError::new(
            ErrorKind::DepthExceeded,
            Stage::Validate,
            backend,
            format!(
                "HTML nesting depth exceeds the conversion budget ({}); \
                 refusing to build a pathological element tree",
                depth::MAX_CONVERSION_DEPTH
            ),
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

    if options.mode == ExtractionMode::Ensemble {
        #[cfg(not(feature = "ensemble"))]
        return Err(crate::model::unsupported_feature(
            "ensemble",
            Backend::Local,
        ));
    }

    let primary_mode = match options.mode {
        ExtractionMode::Ensemble => ExtractionMode::Balanced,
        mode => mode,
    };
    let mut candidates = Vec::new();
    let primary = run_native(
        &prepared_html,
        base_url,
        options,
        primary_mode,
        content_selector,
    );
    let primary_is_weak =
        primary.signals.text_chars < 240 || matches!(primary.signals_band(), QualityBand::Weak);
    candidates.push(primary);

    if options.mode == ExtractionMode::Ensemble {
        candidates.push(run_native(
            &prepared_html,
            base_url,
            options,
            ExtractionMode::Conservative,
            content_selector,
        ));
        candidates.push(run_native(
            &prepared_html,
            base_url,
            options,
            ExtractionMode::Aggressive,
            content_selector,
        ));
    } else if primary_is_weak && primary_mode != ExtractionMode::Conservative {
        candidates.push(run_native(
            &prepared_html,
            base_url,
            options,
            ExtractionMode::Conservative,
            content_selector,
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
    let selected_via_fallback = selected.mode != primary_mode || selected_index != 0;

    let text = crate::native::strip_html_tags(&selected.html);
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

fn run_native(
    html: &str,
    base_url: Option<&Url>,
    options: &ExtractionOptions,
    mode: ExtractionMode,
    content_selector: Option<&str>,
) -> Candidate {
    let started = Instant::now();
    let native_options = crate::native::NativeOptions {
        base_url,
        content_selector,
        include_images: options.include_images,
        include_replies: options.include_replies && mode != ExtractionMode::Aggressive,
        conservative: mode == ExtractionMode::Conservative,
        aggressive: mode == ExtractionMode::Aggressive,
        diagnostics: options.diagnostics,
    };
    let result = crate::native::extract(html, &native_options);
    let sanitized = sanitize::clean(&result.content);

    Candidate {
        signals: analyze(&sanitized),
        html: sanitized,
        metadata: Metadata {
            title: result.metadata.title,
            author: result.metadata.author,
            description: result.metadata.description,
            published: result.metadata.published,
            modified: result.metadata.modified,
            site: result.metadata.site,
            language: result.metadata.language,
            image: result.metadata.image,
            canonical_url: result.metadata.canonical_url,
            keywords: result.metadata.keywords,
        },
        engine: "readabilities-rs".to_string(),
        site_extractor: None,
        removals: result.removals,
        elapsed_ms: elapsed_ms(started),
        mode,
    }
}

fn select_candidate(candidates: &[Candidate]) -> usize {
    let mut selected = 0;
    for (index, candidate) in candidates.iter().enumerate().skip(1) {
        if candidate.signals.score > candidates[selected].signals.score {
            selected = index;
        }
    }
    selected
}

pub(crate) fn analyze(html: &str) -> QualitySignals {
    let text = crate::native::strip_html_tags(html);
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

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
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

    #[test]
    fn pathological_nesting_fails_fast_at_the_depth_screen() {
        let bomb = format!(
            "{}<p>x</p>",
            "<div>".repeat(depth::MAX_CONVERSION_DEPTH + 1)
        );
        let Err(error) = extract(
            &bomb,
            None,
            &ExtractionOptions::default(),
            Backend::Local,
            SnapshotObservations::static_html(crate::SnapshotKind::CallerHtml),
            &[],
        ) else {
            panic!("depth bomb must fail extraction");
        };
        assert_eq!(error.kind, ErrorKind::DepthExceeded);
        assert_eq!(error.stage, Stage::Validate);
    }

    #[test]
    fn reasonably_nested_documents_pass_the_depth_screen() {
        // 400 real levels plus one depth bomb hidden in a comment still passes:
        // comments without internal '<' close properly and never stack.
        let html = format!(
            "<!-- {} -->{}<p>body</p>",
            "-".repeat(600),
            "<section>".repeat(400)
        );
        let result = extract(
            &html,
            None,
            &ExtractionOptions::default(),
            Backend::Local,
            SnapshotObservations::static_html(crate::SnapshotKind::CallerHtml),
            &[],
        );
        // Weak content is allowed to fail extraction later; the failure must
        // never be the depth screen.
        match result {
            Ok(_) => {}
            Err(error) => assert_ne!(error.kind, ErrorKind::DepthExceeded),
        }
    }

    fn candidate_with_score(mode: ExtractionMode, score: i32, text_chars: usize) -> Candidate {
        Candidate {
            html: "<p>candidate</p>".to_string(),
            metadata: Metadata::default(),
            signals: QualitySignals {
                words: 20,
                text_chars,
                paragraphs: 1,
                headings: 0,
                links: 0,
                code_blocks: 0,
                tables: 0,
                score,
            },
            engine: "readabilities-rs".to_string(),
            site_extractor: None,
            removals: Vec::new(),
            elapsed_ms: 0,
            mode,
        }
    }

    #[test]
    fn short_usable_primary_can_lose_to_stronger_native_fallback() {
        let candidates = [
            candidate_with_score(ExtractionMode::Balanced, 10, 100),
            candidate_with_score(ExtractionMode::Conservative, 20, 180),
        ];

        assert_eq!(candidates[0].signals_band(), QualityBand::Usable);
        assert_eq!(select_candidate(&candidates), 1);
    }

    #[test]
    fn candidate_ties_keep_the_earlier_attempt() {
        let candidates = [
            candidate_with_score(ExtractionMode::Balanced, 20, 100),
            candidate_with_score(ExtractionMode::Conservative, 20, 180),
        ];

        assert_eq!(select_candidate(&candidates), 0);
    }

    #[cfg(feature = "ensemble")]
    #[test]
    fn ensemble_compares_only_native_rust_strategies() {
        let options = ExtractionOptions {
            mode: ExtractionMode::Ensemble,
            ..ExtractionOptions::default()
        };
        let result = extract(
            NOISY,
            Some(&Url::parse("https://example.test/story").unwrap()),
            &options,
            Backend::Local,
            SnapshotObservations::static_html(crate::SnapshotKind::CallerHtml),
            &[],
        )
        .unwrap();

        assert_eq!(result.attempts.len(), 3);
        assert!(
            result
                .attempts
                .iter()
                .all(|attempt| attempt.engine == "readabilities-rs")
        );
        assert_eq!(
            result
                .attempts
                .iter()
                .map(|attempt| attempt.mode)
                .collect::<Vec<_>>(),
            vec![
                ExtractionMode::Balanced,
                ExtractionMode::Conservative,
                ExtractionMode::Aggressive,
            ]
        );
    }
}
