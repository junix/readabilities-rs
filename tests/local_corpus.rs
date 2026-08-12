use std::fs;
use std::path::PathBuf;

use readabilities_rs::{OutputFormat, Reader};
use url::Url;

struct Case {
    file: &'static str,
    required: &'static [&'static str],
    forbidden: &'static [&'static str],
}

const CASES: &[Case] = &[
    Case {
        file: "READ-001-noisy-article.html",
        required: &[
            "REQUIRED-CENTRAL-IDEA",
            "REQUIRED-EVIDENCE",
            "REQUIRED-FOOTNOTE",
        ],
        forbidden: &[
            "FORBIDDEN-RELATED",
            "FORBIDDEN-NEWSLETTER",
            "FORBIDDEN-FOOTER",
        ],
    },
    Case {
        file: "READ-002-short-note.html",
        required: &["REQUIRED-SHORT"],
        forbidden: &["FORBIDDEN-SHORT-NAV", "FORBIDDEN-SHORT-FOOTER"],
    },
    Case {
        file: "READ-003-chinese-article.html",
        required: &[
            "必须保留：正文提取的第一步",
            "必须保留：保守的去噪策略",
            "必须保留：最终安全清理",
        ],
        forbidden: &[
            "禁止保留：全站导航",
            "禁止保留：相关推荐",
            "禁止保留：版权信息",
        ],
    },
    Case {
        file: "READ-004-technical-doc.html",
        required: &["REQUIRED-DOC", "REQUIRED-DOC-END"],
        forbidden: &["FORBIDDEN-DOC-NAV", "FORBIDDEN-DOC-FOOTER"],
    },
    Case {
        file: "READ-005-news.html",
        required: &[
            "REQUIRED-NEWS-LEDE",
            "REQUIRED-NEWS-QUOTE",
            "REQUIRED-NEWS-CONTEXT",
        ],
        forbidden: &[
            "FORBIDDEN-BREAKING",
            "FORBIDDEN-TRENDING",
            "FORBIDDEN-NEWS-FOOTER",
        ],
    },
    Case {
        file: "READ-006-blog.html",
        required: &[
            "REQUIRED-BLOG-ONE",
            "REQUIRED-BLOG-LIST",
            "REQUIRED-BLOG-QUOTE",
            "REQUIRED-BLOG-END",
        ],
        forbidden: &[
            "FORBIDDEN-BLOG-HEADER",
            "FORBIDDEN-BLOG-COMMENTS",
            "FORBIDDEN-BLOG-FOOTER",
        ],
    },
    Case {
        file: "READ-007-forum.html",
        required: &["REQUIRED-QUESTION", "REQUIRED-ANSWER", "REQUIRED-REPLY"],
        forbidden: &["FORBIDDEN-FORUM-NAV", "FORBIDDEN-FORUM-RELATED"],
    },
    Case {
        file: "READ-008-math-footnotes.html",
        required: &[
            "REQUIRED-MATH",
            "REQUIRED-MATH-CONTEXT",
            "REQUIRED-MATH-FOOTNOTE",
        ],
        forbidden: &["FORBIDDEN-MATH-NAV", "FORBIDDEN-MATH-SHARE"],
    },
    Case {
        file: "READ-009-images.html",
        required: &["REQUIRED-IMAGE", "REQUIRED-CAPTION", "REQUIRED-IMAGE-END"],
        forbidden: &["FORBIDDEN-IMAGE-AD", "FORBIDDEN-IMAGE-FOOTER"],
    },
    Case {
        file: "READ-010-security.html",
        required: &["REQUIRED-SECURITY", "REQUIRED-SECURITY-STRUCTURE"],
        forbidden: &["FORBIDDEN-SECURITY-NAV", "FORBIDDEN-SECURITY-FOOTER"],
    },
];

fn fixture(name: &str) -> String {
    fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

#[test]
fn frozen_corpus_preserves_required_facts_and_rejects_noise() {
    let reader = Reader::new();
    let base = Url::parse("https://example.test/article").unwrap();
    for case in CASES {
        let article = reader
            .extract_html(&fixture(case.file), Some(&base))
            .unwrap_or_else(|error| panic!("{} failed: {error}", case.file));
        for required in case.required {
            assert!(
                article.content.contains(required),
                "{} omitted required fact {required:?}\n{}",
                case.file,
                article.content
            );
        }
        for forbidden in case.forbidden {
            assert!(
                !article.content.contains(forbidden),
                "{} retained forbidden fact {forbidden:?}\n{}",
                case.file,
                article.content
            );
        }
        let lower = article.content.to_ascii_lowercase();
        assert!(!lower.contains("<script"), "{} retained script", case.file);
        assert!(
            !lower.contains("javascript:"),
            "{} retained javascript URI",
            case.file
        );
        assert!(
            !lower.contains("data:text/html"),
            "{} retained data URI",
            case.file
        );
        assert!(
            !lower.contains(" onclick="),
            "{} retained onclick",
            case.file
        );
        assert!(
            !lower.contains(" onerror="),
            "{} retained onerror",
            case.file
        );
    }
}

#[test]
fn rich_structure_and_relative_urls_survive() {
    let base = Url::parse("https://example.test/local-first-reading").unwrap();
    let article = Reader::new()
        .extract_html(&fixture("READ-001-noisy-article.html"), Some(&base))
        .unwrap();
    for tag in ["<h1", "<h2", "<pre", "<code", "<table", "<img", "<sup"] {
        assert!(
            article.content.contains(tag),
            "missing {tag}: {}",
            article.content
        );
    }
    assert!(
        article
            .content
            .contains("https://example.test/images/pipeline.png")
    );
    assert!(article.content.contains("https://example.test/details"));
    assert_eq!(article.metadata.author.as_deref(), Some("Ada Example"));
    assert_eq!(article.metadata.site.as_deref(), Some("Example Journal"));
    assert_eq!(article.metadata.language.as_deref(), Some("en"));
}

#[test]
fn renderers_do_not_rerun_or_mutate_extraction() {
    let article = Reader::new()
        .extract_html(&fixture("READ-004-technical-doc.html"), None)
        .unwrap();
    let canonical = article.content.clone();
    let markdown = article.render(OutputFormat::Markdown).unwrap();
    let text = article.render(OutputFormat::Text).unwrap();
    let json = article.render(OutputFormat::Json).unwrap();
    assert_eq!(article.content, canonical);
    assert!(markdown.contains("# Parser API Guide"));
    assert!(markdown.contains("```"));
    assert!(text.contains("REQUIRED-DOC"));
    assert!(json.contains("\"schema_version\": 1"));
}

#[test]
fn link_index_is_retained_when_the_oracle_treats_it_as_readable_content() {
    let article = Reader::new()
        .extract_html(&fixture("READ-017-link-index.html"), None)
        .expect("the Defuddle-aligned link index should be extracted");
    assert!(article.content.contains("A report on reliable extraction"));
    assert!(
        article
            .content
            .contains("A visual guide to distributed tracing")
    );
}
