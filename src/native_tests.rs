use super::*;

#[test]
fn selects_article_and_removes_nested_noise() {
    let html = r#"<html><head><title>Native</title></head><body><nav>noise</nav>
        <main><article><h1>Native</h1><p>Keep this meaningful paragraph.</p>
        <div class="related-posts">remove this</div></article></main></body></html>"#;
    let result = extract(
        html,
        &NativeOptions {
            base_url: None,
            content_selector: None,
            include_images: true,
            include_replies: true,
            conservative: false,
            aggressive: false,
            diagnostics: true,
        },
    );

    assert!(result.content.contains("Keep this meaningful paragraph"));
    assert!(!result.content.contains("remove this"));
    assert_eq!(result.metadata.title.as_deref(), Some("Native"));
}

#[test]
fn resolves_relative_urls_without_network_access() {
    let base = Url::parse("https://example.test/posts/one").unwrap();
    let result = extract(
        r#"<article><p><a href="/detail">detail</a><img src="image.png"></p></article>"#,
        &NativeOptions {
            base_url: Some(&base),
            content_selector: None,
            include_images: true,
            include_replies: true,
            conservative: false,
            aggressive: false,
            diagnostics: false,
        },
    );

    assert!(result.content.contains("https://example.test/detail"));
    assert!(
        result
            .content
            .contains("https://example.test/posts/image.png")
    );
}

#[test]
fn document_base_and_json_ld_enrich_the_native_result() {
    let base = Url::parse("https://example.test/posts/one").unwrap();
    let html = r#"
        <html><head><base href="/assets/">
        <meta name="description" content="Explicit head description">
        <script type="application/ld+json">{
          "@context": "https://schema.org",
          "@graph": [
            {"@type": "WebSite", "name": "Fallback Site"},
            {
              "@type": ["NewsArticle", "Report"],
              "headline": "Structured title",
              "author": [{"name": "Ada"}, {"name": "Lin"}],
              "description": "Structured description",
              "datePublished": "2026-08-12",
              "publisher": {"name": "Example Journal"},
              "image": {"url": "hero.jpg"},
              "mainEntityOfPage": {"@id": "story"},
              "keywords": ["rust", "readability"]
            }
          ]
        }</script></head><body><article>
          <p><a href="detail">detail</a><img src="image.png"></p>
        </article></body></html>
    "#;
    let result = extract(
        html,
        &NativeOptions {
            base_url: Some(&base),
            content_selector: None,
            include_images: true,
            include_replies: true,
            conservative: false,
            aggressive: false,
            diagnostics: false,
        },
    );

    assert!(
        result
            .content
            .contains("https://example.test/assets/detail")
    );
    assert!(
        result
            .content
            .contains("https://example.test/assets/image.png")
    );
    assert_eq!(result.metadata.title.as_deref(), Some("Structured title"));
    assert_eq!(result.metadata.author.as_deref(), Some("Ada, Lin"));
    assert_eq!(
        result.metadata.description.as_deref(),
        Some("Explicit head description")
    );
    assert_eq!(result.metadata.site.as_deref(), Some("Example Journal"));
    assert_eq!(
        result.metadata.image.as_deref(),
        Some("https://example.test/assets/hero.jpg")
    );
    assert_eq!(
        result.metadata.canonical_url.as_deref(),
        Some("https://example.test/assets/story")
    );
    assert_eq!(result.metadata.keywords, ["rust", "readability"]);
}

#[test]
fn explicit_head_metadata_precedes_json_ld_fallbacks() {
    let result = extract(
        r#"<html><head>
          <meta name="keywords" content="head-topic, rust">
          <script type="application/ld+json">{
            "@type": "Article",
            "keywords": ["json-topic", "rust"]
          }</script>
        </head><body><article><p>Enough article text for metadata extraction.</p></article></body></html>"#,
        &NativeOptions {
            base_url: None,
            content_selector: None,
            include_images: true,
            include_replies: true,
            conservative: false,
            aggressive: false,
            diagnostics: false,
        },
    );

    assert_eq!(result.metadata.keywords, ["head-topic", "rust"]);
}

#[test]
fn metadata_names_are_case_insensitive() {
    let result = extract(
        r#"<html lang="en"><head>
          <title>Generic repository title</title>
          <meta name="DC.title" content="Effects on Marine Biodiversity">
          <meta name="DC.creator" content="Dr. Jane Smith">
          <meta name="DC.subject" content="Marine Biology; Climate Change; Biodiversity">
          <meta name="DC.description" content="A comprehensive marine study.">
          <meta name="DC.publisher" content="Academic Press">
          <meta name="DC.date" content="2025-03-01">
          <meta name="DC.identifier" content="/papers/marine-study">
          <meta PROPERTY="OG:IMAGE" content="/images/hero.jpg">
        </head><body><article>
          <h1>Effects on Marine Biodiversity</h1>
          <p>A comprehensive marine study with enough readable content.</p>
        </article></body></html>"#,
        &NativeOptions {
            base_url: Some(&Url::parse("https://example.test/source").unwrap()),
            content_selector: None,
            include_images: true,
            include_replies: true,
            conservative: false,
            aggressive: false,
            diagnostics: false,
        },
    );

    assert_eq!(
        result.metadata.title.as_deref(),
        Some("Effects on Marine Biodiversity")
    );
    assert_eq!(result.metadata.author.as_deref(), Some("Dr. Jane Smith"));
    assert_eq!(
        result.metadata.description.as_deref(),
        Some("A comprehensive marine study.")
    );
    assert_eq!(result.metadata.published.as_deref(), Some("2025-03-01"));
    assert_eq!(result.metadata.site.as_deref(), Some("Academic Press"));
    assert_eq!(
        result.metadata.canonical_url.as_deref(),
        Some("https://example.test/papers/marine-study")
    );
    assert_eq!(
        result.metadata.image.as_deref(),
        Some("https://example.test/images/hero.jpg")
    );
    assert_eq!(
        result.metadata.keywords,
        ["Marine Biology", "Climate Change", "Biodiversity"]
    );
}
