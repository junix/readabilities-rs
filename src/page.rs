use std::collections::BTreeMap;

#[cfg(feature = "charset")]
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{Article, ReadError, SnapshotObservations};

/// Raw bytes and acquisition facts for one already-fetched page.
///
/// This is the integration boundary for crawlers: acquisition happens once in
/// the caller, while decoding and page understanding stay in `readabilities-rs`.
#[derive(Debug, Clone)]
pub struct PageSnapshot {
    pub final_url: Url,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    pub observations: SnapshotObservations,
    pub response_headers: BTreeMap<String, String>,
}

impl PageSnapshot {
    pub fn origin(final_url: Url, content_type: Option<String>, body: Vec<u8>) -> Self {
        Self {
            final_url,
            content_type,
            body,
            observations: SnapshotObservations::origin_response(),
            response_headers: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_response_header(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.response_headers
            .insert(name.into().to_ascii_lowercase(), value.into());
        self
    }
}

/// A navigable link discovered from the full source document, before article
/// selection removes navigation and page chrome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredLink {
    pub url: Url,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rel: Vec<String>,
    pub nofollow: bool,
}

/// Page-level robots directives discovered from `<meta name="robots">`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetaRobots {
    pub noindex: bool,
    pub nofollow: bool,
}

/// Results derived from one immutable page snapshot.
///
/// Link discovery is independent from article extraction. A page can therefore
/// remain useful to a crawler even when it has no readable article.
#[derive(Debug, Clone)]
pub struct PageAnalysis {
    pub article: std::result::Result<Article, ReadError>,
    pub links: Vec<DiscoveredLink>,
    pub canonical_url: Option<Url>,
    pub robots: MetaRobots,
    pub detected_encoding: String,
    pub decode_errors: bool,
}

#[cfg(feature = "charset")]
pub(crate) struct PageFacts {
    pub links: Vec<DiscoveredLink>,
    pub canonical_url: Option<Url>,
    pub robots: MetaRobots,
}

#[cfg(feature = "charset")]
pub(crate) fn inspect(html: &str, document_url: &Url) -> PageFacts {
    let document = Html::parse_document(html);
    let effective_base = first_attr(&document, "base[href]", "href")
        .and_then(|value| document_url.join(value).ok())
        .unwrap_or_else(|| document_url.clone());

    let canonical_url = first_attr(&document, "link[rel][href]", "href").and_then(|_| {
        let selector = Selector::parse("link[rel][href]").expect("static selector must be valid");
        document.select(&selector).find_map(|element| {
            let canonical = rel_tokens(element.attr("rel").unwrap_or_default())
                .iter()
                .any(|token| token == "canonical");
            canonical
                .then(|| element.attr("href"))
                .flatten()
                .and_then(|value| effective_base.join(value.trim()).ok())
        })
    });

    let mut robots = MetaRobots::default();
    let meta_selector =
        Selector::parse("meta[name][content]").expect("static selector must be valid");
    for element in document.select(&meta_selector) {
        let name = element.attr("name").unwrap_or_default();
        if !matches!(
            name.to_ascii_lowercase().as_str(),
            "robots" | "googlebot" | "bingbot"
        ) {
            continue;
        }
        for directive in element
            .attr("content")
            .unwrap_or_default()
            .split([',', ';'])
            .map(str::trim)
        {
            if directive.eq_ignore_ascii_case("noindex") {
                robots.noindex = true;
            } else if directive.eq_ignore_ascii_case("nofollow") {
                robots.nofollow = true;
            } else if directive.eq_ignore_ascii_case("none") {
                robots.noindex = true;
                robots.nofollow = true;
            }
        }
    }

    let link_selector = Selector::parse("a[href]").expect("static selector must be valid");
    let links = document
        .select(&link_selector)
        .filter_map(|element| {
            let href = element.attr("href")?.trim();
            if href.is_empty() || href.starts_with('#') {
                return None;
            }
            let url = effective_base.join(href).ok()?;
            if !matches!(url.scheme(), "http" | "https") {
                return None;
            }
            let rel = rel_tokens(element.attr("rel").unwrap_or_default());
            let nofollow = rel.iter().any(|token| token == "nofollow");
            let text = element
                .text()
                .collect::<Vec<_>>()
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            Some(DiscoveredLink {
                url,
                text,
                rel,
                nofollow,
            })
        })
        .collect();

    PageFacts {
        links,
        canonical_url,
        robots,
    }
}

#[cfg(feature = "charset")]
pub(crate) fn apply_x_robots_tag(robots: &mut MetaRobots, value: Option<&str>) {
    for directive in value.unwrap_or_default().split([',', ';']).map(str::trim) {
        if directive.eq_ignore_ascii_case("noindex") {
            robots.noindex = true;
        } else if directive.eq_ignore_ascii_case("nofollow") {
            robots.nofollow = true;
        } else if directive.eq_ignore_ascii_case("none") {
            robots.noindex = true;
            robots.nofollow = true;
        }
    }
}

#[cfg(feature = "charset")]
pub(crate) fn looks_like_html(content_type: Option<&str>, body: &[u8]) -> bool {
    if content_type.is_some_and(|value| {
        let value = value.to_ascii_lowercase();
        value.contains("text/html") || value.contains("application/xhtml+xml")
    }) {
        return true;
    }
    let sample = &body[..body.len().min(1_024)];
    let sample = String::from_utf8_lossy(sample).to_ascii_lowercase();
    let sample = sample.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
    sample.starts_with("<!doctype html")
        || sample.starts_with("<html")
        || sample.contains("<head")
        || sample.contains("<body")
        || sample.contains("<article")
        || sample.contains("<main")
}

#[cfg(feature = "charset")]
fn first_attr<'a>(document: &'a Html, raw_selector: &str, attribute: &str) -> Option<&'a str> {
    let selector = Selector::parse(raw_selector).ok()?;
    document.select(&selector).next()?.attr(attribute)
}

#[cfg(feature = "charset")]
fn rel_tokens(value: &str) -> Vec<String> {
    value
        .split_ascii_whitespace()
        .map(str::to_ascii_lowercase)
        .collect()
}

#[cfg(all(test, feature = "charset"))]
#[path = "page_tests.rs"]
mod tests;
