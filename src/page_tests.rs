use super::*;

#[test]
fn full_page_links_are_independent_from_article_content() {
    let facts = inspect(
        r##"<html><head><base href="/docs/"><link rel="CANONICAL" href="index">
           <meta name="ROBOTS" content="noindex, nofollow"></head><body>
           <nav><a href="next" rel="nofollow"> Next page </a></nav>
           <article><p>Readable article.</p></article>
           <a href="mailto:test@example.com">mail</a><a href="#part">part</a>
           </body></html>"##,
        &Url::parse("https://example.test/start").unwrap(),
    );

    assert_eq!(facts.links.len(), 1);
    assert_eq!(
        facts.links[0].url.as_str(),
        "https://example.test/docs/next"
    );
    assert!(facts.links[0].nofollow);
    assert_eq!(facts.links[0].text, "Next page");
    assert_eq!(
        facts.canonical_url.unwrap().as_str(),
        "https://example.test/docs/index"
    );
    assert_eq!(
        facts.robots,
        MetaRobots {
            noindex: true,
            nofollow: true
        }
    );
}

#[test]
fn media_type_sniffing_and_header_robots_are_bounded() {
    assert!(!looks_like_html(Some("application/pdf"), b"%PDF-1.7"));
    assert!(looks_like_html(
        Some("application/octet-stream"),
        b"<!doctype html><html></html>"
    ));
    let mut robots = MetaRobots::default();
    apply_x_robots_tag(&mut robots, Some("none"));
    assert!(robots.noindex && robots.nofollow);
}
