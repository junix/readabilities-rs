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
