//! Native readable-content extraction.
//!
//! Defuddle is the behavioral reference for pipeline ordering and heuristics.
//! This module implements those ideas directly in Rust; it does not invoke
//! Defuddle, another extraction crate, a subprocess, or a remote reader service.

use std::collections::HashSet;

use ego_tree::NodeId;
use scraper::{ElementRef, Html, Node, Selector};
use serde_json::{Map, Value};
use unicode_segmentation::UnicodeSegmentation;
use url::Url;

use crate::model::{Metadata, RemovalRecord};

const ENTRY_POINTS: &[&str] = &[
    "#post",
    ".post-content",
    ".post-body",
    ".article-content",
    "#article-content",
    ".entry-content",
    ".markdown-body",
    "article",
    "[role=article]",
    "main",
    "[role=main]",
    ".article-body",
    "#content",
    "body",
];

const EXACT_NOISE: &str = r#"
    script:not([type^="math/"]), style, noscript, template, meta, link,
    nav, footer, header:not(:has(p + p)):not(:has(img)),
    aside:not([class*="callout"]), form, dialog, object, embed, applet,
    canvas, button, select, textarea, [role="navigation"],
    [role="dialog"], [role="alertdialog"], [role="complementary"]
"#;

const NOISE_MARKERS: &[&str] = &[
    "advert",
    "banner",
    "breaking",
    "catlinks",
    "clap",
    "comments",
    "cookie",
    "copyright",
    "editsection",
    "floating-tools",
    "footer",
    "hero-ad",
    "menu",
    "navbox",
    "newsletter",
    "page-footer",
    "post-preview",
    "prev-next",
    "promo",
    "recommend",
    "related",
    "share",
    "sidebar",
    "social",
    "sponsor",
    "subscribe",
    "survey",
    "trending",
    "widget",
];

pub(crate) struct NativeOptions<'a> {
    pub base_url: Option<&'a Url>,
    pub content_selector: Option<&'a str>,
    pub include_images: bool,
    pub include_replies: bool,
    pub conservative: bool,
    pub aggressive: bool,
    pub diagnostics: bool,
}

pub(crate) struct NativeResult {
    pub content: String,
    pub metadata: Metadata,
    pub removals: Vec<RemovalRecord>,
}

#[derive(Clone, Copy)]
struct ContentCandidate {
    id: NodeId,
    score: i64,
    selector_index: usize,
    words: usize,
}

pub(crate) fn extract(html: &str, options: &NativeOptions<'_>) -> NativeResult {
    let mut document = Html::parse_document(html);
    let effective_base = effective_base_url(&document, options.base_url);
    let base_url = effective_base.as_ref().or(options.base_url);
    let metadata = extract_metadata(&document, base_url);
    let root_id = find_content_root(&document, options.content_selector)
        .unwrap_or_else(|| document.root_element().id());
    let mut removals = Vec::new();

    remove_exact_noise(&mut document, root_id, options, &mut removals);
    if !options.conservative {
        remove_attribute_noise(&mut document, root_id, options, &mut removals);
    }
    if !options.include_images {
        remove_matching(
            &mut document,
            root_id,
            "img, picture, source",
            "images disabled",
            options.diagnostics,
            &mut removals,
        );
    }
    resolve_relative_urls(&mut document, root_id, base_url);

    let content = document
        .tree
        .get(root_id)
        .and_then(ElementRef::wrap)
        .map_or_else(String::new, |element| element.html());

    NativeResult {
        content,
        metadata,
        removals,
    }
}

pub(crate) fn strip_html_tags(html: &str) -> String {
    Html::parse_fragment(html)
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join(" ")
}

fn find_content_root(document: &Html, requested: Option<&str>) -> Option<NodeId> {
    if let Some(raw) = requested
        && let Ok(selector) = Selector::parse(raw)
        && let Some(element) = document.select(&selector).next()
    {
        return Some(element.id());
    }

    let mut candidates = Vec::<ContentCandidate>::new();
    for (selector_index, raw) in ENTRY_POINTS.iter().enumerate() {
        let Ok(selector) = Selector::parse(raw) else {
            continue;
        };
        for element in document.select(&selector) {
            let words = count_words(&element.text().collect::<Vec<_>>().join(" "));
            let score = i64::try_from((ENTRY_POINTS.len() - selector_index) * 40)
                .unwrap_or(i64::MAX / 2)
                + score_element(element);
            let candidate = ContentCandidate {
                id: element.id(),
                score,
                selector_index,
                words,
            };
            if let Some(existing) = candidates.iter_mut().find(|item| item.id == candidate.id) {
                if candidate.selector_index < existing.selector_index {
                    *existing = candidate;
                }
            } else {
                candidates.push(candidate);
            }
        }
    }
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.score));
    let top = *candidates.first()?;
    let mut best = top;

    for child in candidates.iter().copied().skip(1) {
        if child.selector_index >= best.selector_index || child.words <= 50 {
            continue;
        }
        if !is_descendant(document, child.id, best.id) {
            continue;
        }
        let peers = candidates
            .iter()
            .filter(|candidate| {
                candidate.selector_index == child.selector_index
                    && is_descendant(document, candidate.id, top.id)
            })
            .count();
        if peers == 1 {
            best = child;
        }
    }
    Some(best.id)
}

fn score_element(element: ElementRef<'_>) -> i64 {
    let text = element.text().collect::<Vec<_>>().join(" ");
    let words = count_words(&text);
    let paragraphs = count_selector(element, "p");
    let images = count_selector(element, "img");
    let tables = count_selector(element, "table");
    let commas = text.matches(',').count();
    let link_selector = Selector::parse("a").expect("static selector must be valid");
    let link_chars = element
        .select(&link_selector)
        .map(|link| link.text().map(str::len).sum::<usize>())
        .sum::<usize>();
    let text_chars = text.chars().count().max(1);
    let density_per_mille = (link_chars.saturating_mul(1_000) / text_chars).min(500);

    let mut score = i64::try_from(words + paragraphs * 10 + commas).unwrap_or(i64::MAX / 4);
    score -= i64::try_from(images.saturating_mul(3)).unwrap_or(0);
    score -= i64::try_from(tables.saturating_mul(5)).unwrap_or(0);
    let attributes = relevant_attributes(element);
    if ["content", "article", "post", "entry", "story"]
        .iter()
        .any(|marker| attributes.contains(marker))
    {
        score += 15;
    }
    score * i64::try_from(1_000 - density_per_mille).unwrap_or(500) / 1_000
}

fn remove_exact_noise(
    document: &mut Html,
    root_id: NodeId,
    options: &NativeOptions<'_>,
    removals: &mut Vec<RemovalRecord>,
) {
    remove_matching(
        document,
        root_id,
        EXACT_NOISE,
        "exact non-content selector",
        options.diagnostics,
        removals,
    );
    remove_matching(
        document,
        root_id,
        "iframe, frame, frameset",
        "active embedded content",
        options.diagnostics,
        removals,
    );
    if !options.include_replies {
        remove_matching(
            document,
            root_id,
            ".reply, .replies, [data-comment], [data-reply]",
            "replies disabled",
            options.diagnostics,
            removals,
        );
    }
}

fn remove_attribute_noise(
    document: &mut Html,
    root_id: NodeId,
    options: &NativeOptions<'_>,
    removals: &mut Vec<RemovalRecord>,
) {
    let ids = document
        .tree
        .get(root_id)
        .and_then(ElementRef::wrap)
        .map(|root| {
            root.descendent_elements()
                .filter(|element| should_remove_element(*element, options))
                .map(|element| element.id())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    detach_nodes(
        document,
        ids,
        "partial non-content marker",
        options.diagnostics,
        removals,
    );
}

fn should_remove_element(element: ElementRef<'_>, options: &NativeOptions<'_>) -> bool {
    let tag = element.value().name();
    if matches!(
        tag,
        "article"
            | "main"
            | "pre"
            | "code"
            | "table"
            | "thead"
            | "tbody"
            | "tr"
            | "td"
            | "th"
            | "figure"
            | "picture"
            | "math"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
    ) {
        return false;
    }
    if is_hidden(element) {
        return true;
    }
    let attributes = relevant_attributes(element);
    if options.include_replies && has_marker(&attributes, "reply") {
        return false;
    }
    if attributes.split_whitespace().any(|value| value == "ad")
        || attributes.contains("ad-")
        || attributes.contains("-ad")
    {
        return true;
    }
    let noisy = NOISE_MARKERS
        .iter()
        .any(|marker| has_marker(&attributes, marker));
    if !noisy {
        return false;
    }
    let text = element.text().collect::<Vec<_>>().join(" ");
    let words = count_words(&text);
    let content_selector =
        Selector::parse("p, pre, table, figure, math").expect("static selector must be valid");
    let contains_content = element.select(&content_selector).next().is_some();
    if contains_content && words >= if options.aggressive { 200 } else { 100 } {
        return false;
    }
    true
}

fn is_hidden(element: ElementRef<'_>) -> bool {
    if element.attr("hidden").is_some()
        || element
            .attr("aria-hidden")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
    {
        return true;
    }
    let style = element
        .attr("style")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace(' ', "");
    if style.contains("display:none")
        || style.contains("visibility:hidden")
        || style.contains("opacity:0")
    {
        return true;
    }
    element.attr("class").is_some_and(|classes| {
        let classes = classes.to_ascii_lowercase();
        let responsive = classes
            .split_whitespace()
            .any(|class| class.contains(':') && !class.ends_with(":hidden"));
        !responsive
            && classes
                .split_whitespace()
                .any(|class| matches!(class, "hidden" | "invisible" | "is-hidden"))
    })
}

fn remove_matching(
    document: &mut Html,
    root_id: NodeId,
    raw_selector: &str,
    reason: &str,
    diagnostics: bool,
    removals: &mut Vec<RemovalRecord>,
) {
    let Ok(selector) = Selector::parse(raw_selector) else {
        return;
    };
    let ids = document
        .tree
        .get(root_id)
        .and_then(ElementRef::wrap)
        .map(|root| root.select(&selector).map(|element| element.id()).collect())
        .unwrap_or_default();
    detach_nodes(document, ids, reason, diagnostics, removals);
}

fn detach_nodes(
    document: &mut Html,
    ids: Vec<NodeId>,
    reason: &str,
    diagnostics: bool,
    removals: &mut Vec<RemovalRecord>,
) {
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(id) {
            continue;
        }
        if diagnostics
            && let Some(element) = document.tree.get(id).and_then(ElementRef::wrap)
            && removals.len() < 200
        {
            removals.push(RemovalRecord {
                step: "native_cleanup".to_string(),
                selector: Some(element.value().name().to_string()),
                reason: Some(reason.to_string()),
                preview: element
                    .text()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(200)
                    .collect(),
            });
        }
        if let Some(mut node) = document.tree.get_mut(id) {
            node.detach();
        }
    }
}

fn resolve_relative_urls(document: &mut Html, root_id: NodeId, base_url: Option<&Url>) {
    let Some(base_url) = base_url else {
        return;
    };
    let ids = document
        .tree
        .get(root_id)
        .and_then(ElementRef::wrap)
        .map(|root| {
            std::iter::once(root.id())
                .chain(root.descendent_elements().map(|element| element.id()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    for id in ids {
        let Some(mut node) = document.tree.get_mut(id) else {
            continue;
        };
        let Node::Element(element) = node.value() else {
            continue;
        };
        for (name, value) in &mut element.attrs {
            let attribute = name.local.as_ref();
            if matches!(
                attribute,
                "href" | "src" | "poster" | "action" | "formaction"
            ) {
                if let Ok(resolved) = base_url.join(value.as_ref()) {
                    *value = resolved.as_str().into();
                }
            } else if attribute == "srcset" {
                let resolved = value
                    .split(',')
                    .map(|entry| {
                        let mut parts = entry.split_whitespace();
                        let raw = parts.next().unwrap_or_default();
                        let suffix = parts.collect::<Vec<_>>().join(" ");
                        let url = base_url
                            .join(raw)
                            .map_or_else(|_| raw.to_string(), |url| url.to_string());
                        if suffix.is_empty() {
                            url
                        } else {
                            format!("{url} {suffix}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                *value = resolved.into();
            }
        }
    }
}

fn effective_base_url(document: &Html, fallback: Option<&Url>) -> Option<Url> {
    let declared = select_first(document, "base[href]")
        .and_then(|element| element.attr("href"))
        .map(str::trim)
        .filter(|value| !value.is_empty());
    declared
        .and_then(|value| fallback.and_then(|base| base.join(value).ok()))
        .or_else(|| declared.and_then(|value| Url::parse(value).ok()))
        .or_else(|| fallback.cloned())
}

fn extract_metadata(document: &Html, base_url: Option<&Url>) -> Metadata {
    let meta = collect_meta_values(document);
    let title = meta_content(&meta, &["og:title", "twitter:title", "dc.title"])
        .or_else(|| selector_text(document, "title"));
    let author = meta_content(
        &meta,
        &[
            "author",
            "article:author",
            "byl",
            "twitter:creator",
            "dc.creator",
        ],
    );
    let description = meta_content(
        &meta,
        &[
            "description",
            "og:description",
            "twitter:description",
            "dc.description",
        ],
    );
    let published = meta_content(
        &meta,
        &["article:published_time", "date", "pubdate", "dc.date"],
    );
    let modified = meta_content(&meta, &["article:modified_time", "last-modified"]);
    let site = meta_content(&meta, &["og:site_name", "application-name", "dc.publisher"]);
    let language = select_first(document, "html")
        .and_then(|element| element.attr("lang"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| meta_content(&meta, &["dc.language", "og:locale"]));
    let image = meta_content(&meta, &["og:image", "twitter:image"])
        .map(|value| resolve_url(&value, base_url));
    let canonical_url = select_first(document, "link[rel=\"canonical\"]")
        .and_then(|element| element.attr("href"))
        .map(|value| resolve_url(value, base_url))
        .or_else(|| {
            meta_content(&meta, &["og:url", "dc.identifier"])
                .map(|value| resolve_url(&value, base_url))
        });
    let mut keywords = meta_contents(&meta, &["keywords", "article:tag"])
        .into_iter()
        .flat_map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .chain(
            meta_contents(&meta, &["dc.subject"])
                .into_iter()
                .flat_map(|value| {
                    value
                        .split([',', ';'])
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned)
                        .collect::<Vec<_>>()
                }),
        )
        .collect::<Vec<_>>();
    deduplicate(&mut keywords);

    let mut metadata = Metadata {
        title,
        author,
        description,
        published,
        modified,
        site,
        language,
        image,
        canonical_url,
        keywords,
    };
    let json_ld = extract_json_ld_metadata(document, base_url);
    merge_missing_metadata(&mut metadata, json_ld);
    metadata
}

fn collect_meta_values(document: &Html) -> Vec<(String, String)> {
    let selector = Selector::parse("meta").expect("static selector must be valid");
    document
        .select(&selector)
        .filter_map(|element| {
            let key = element
                .attr("name")
                .or_else(|| element.attr("property"))?
                .trim();
            let content = element.attr("content")?.trim();
            (!key.is_empty() && !content.is_empty())
                .then(|| (key.to_ascii_lowercase(), content.to_string()))
        })
        .collect()
}

fn meta_content(values: &[(String, String)], keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        values
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value.clone())
    })
}

fn meta_contents(values: &[(String, String)], keys: &[&str]) -> Vec<String> {
    keys.iter()
        .flat_map(|key| {
            values
                .iter()
                .filter(move |(candidate, _)| candidate == key)
                .map(|(_, value)| value.clone())
        })
        .collect()
}

fn extract_json_ld_metadata(document: &Html, base_url: Option<&Url>) -> Metadata {
    let Ok(selector) = Selector::parse("script[type=\"application/ld+json\"]") else {
        return Metadata::default();
    };
    let mut candidates = Vec::new();
    for element in document.select(&selector) {
        if let Ok(value) = serde_json::from_str::<Value>(&element.inner_html()) {
            collect_json_ld_candidates(&value, base_url, &mut candidates);
        }
    }
    candidates.sort_by_key(|(priority, _)| std::cmp::Reverse(*priority));
    let mut metadata = Metadata::default();
    for (_, candidate) in candidates {
        merge_missing_metadata(&mut metadata, candidate);
    }
    metadata
}

fn collect_json_ld_candidates(
    value: &Value,
    base_url: Option<&Url>,
    candidates: &mut Vec<(u8, Metadata)>,
) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_json_ld_candidates(value, base_url, candidates);
            }
        }
        Value::Object(object) => {
            if let Some(graph) = object.get("@graph") {
                collect_json_ld_candidates(graph, base_url, candidates);
            }
            let metadata = metadata_from_json_ld_object(object, base_url);
            if metadata != Metadata::default() {
                candidates.push((json_ld_priority(object), metadata));
            }
        }
        _ => {}
    }
}

fn metadata_from_json_ld_object(object: &Map<String, Value>, base_url: Option<&Url>) -> Metadata {
    let title = json_string(object.get("headline")).or_else(|| json_string(object.get("name")));
    let author = json_names(object.get("author"));
    let description = json_string(object.get("description"));
    let published = json_string(object.get("datePublished"));
    let modified = json_string(object.get("dateModified"));
    let site = json_names(object.get("publisher")).or_else(|| json_names(object.get("isPartOf")));
    let language = json_string(object.get("inLanguage"));
    let image = json_url(object.get("image"), &["url", "contentUrl", "@id"])
        .map(|value| resolve_url(&value, base_url));
    let canonical_url = json_url(object.get("mainEntityOfPage"), &["@id", "url"])
        .or_else(|| json_url(object.get("url"), &["@id", "url"]))
        .map(|value| resolve_url(&value, base_url));
    let mut keywords = json_keywords(object.get("keywords"));
    deduplicate(&mut keywords);

    Metadata {
        title,
        author,
        description,
        published,
        modified,
        site,
        language,
        image,
        canonical_url,
        keywords,
    }
}

fn json_ld_priority(object: &Map<String, Value>) -> u8 {
    let types = match object.get("@type") {
        Some(Value::String(value)) => vec![value.as_str()],
        Some(Value::Array(values)) => values.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    if types.iter().any(|value| {
        ["article", "posting", "report", "review", "recipe"]
            .iter()
            .any(|kind| value.to_ascii_lowercase().contains(kind))
    }) {
        2
    } else {
        u8::from(
            types
                .iter()
                .any(|value| value.eq_ignore_ascii_case("WebPage")),
        )
    }
}

fn json_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => nonempty(value),
        Value::Object(object) => object
            .get("name")
            .or_else(|| object.get("@value"))
            .and_then(|value| json_string(Some(value))),
        Value::Array(values) => values.iter().find_map(|value| json_string(Some(value))),
        _ => None,
    }
}

fn json_names(value: Option<&Value>) -> Option<String> {
    let values = match value? {
        Value::Array(values) => values
            .iter()
            .filter_map(|value| json_string(Some(value)))
            .collect::<Vec<_>>(),
        value => json_string(Some(value)).into_iter().collect(),
    };
    (!values.is_empty()).then(|| values.join(", "))
}

fn json_url(value: Option<&Value>, object_keys: &[&str]) -> Option<String> {
    match value? {
        Value::String(value) => nonempty(value),
        Value::Object(object) => object_keys
            .iter()
            .find_map(|key| json_string(object.get(*key))),
        Value::Array(values) => values
            .iter()
            .find_map(|value| json_url(Some(value), object_keys)),
        _ => None,
    }
}

fn json_keywords(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(value)) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(|value| json_string(Some(value)))
            .collect(),
        _ => Vec::new(),
    }
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn merge_missing_metadata(target: &mut Metadata, fallback: Metadata) {
    if target.title.is_none() {
        target.title = fallback.title;
    }
    if target.author.is_none() {
        target.author = fallback.author;
    }
    if target.description.is_none() {
        target.description = fallback.description;
    }
    if target.published.is_none() {
        target.published = fallback.published;
    }
    if target.modified.is_none() {
        target.modified = fallback.modified;
    }
    if target.site.is_none() {
        target.site = fallback.site;
    }
    if target.language.is_none() {
        target.language = fallback.language;
    }
    if target.image.is_none() {
        target.image = fallback.image;
    }
    if target.canonical_url.is_none() {
        target.canonical_url = fallback.canonical_url;
    }
    if target.keywords.is_empty() {
        target.keywords = fallback.keywords;
        deduplicate(&mut target.keywords);
    }
}

fn deduplicate(values: &mut Vec<String>) {
    let mut seen = HashSet::new();
    values.retain(|value| seen.insert(value.to_ascii_lowercase()));
}

fn selector_text(document: &Html, raw: &str) -> Option<String> {
    let text = select_first(document, raw)?
        .text()
        .collect::<Vec<_>>()
        .join(" ");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn select_first<'a>(document: &'a Html, raw: &str) -> Option<ElementRef<'a>> {
    let selector = Selector::parse(raw).ok()?;
    document.select(&selector).next()
}

fn count_selector(element: ElementRef<'_>, raw: &str) -> usize {
    let selector = Selector::parse(raw).expect("static selector must be valid");
    element.select(&selector).count()
}

fn relevant_attributes(element: ElementRef<'_>) -> String {
    [
        "id",
        "class",
        "role",
        "data-component",
        "data-test",
        "data-testid",
        "data-test-id",
        "data-qa",
        "data-cy",
    ]
    .iter()
    .filter_map(|name| element.attr(name))
    .collect::<Vec<_>>()
    .join(" ")
    .to_ascii_lowercase()
}

fn has_marker(attributes: &str, marker: &str) -> bool {
    attributes
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|part| part == marker || part.starts_with(marker))
        || attributes.contains(marker)
}

fn resolve_url(value: &str, base_url: Option<&Url>) -> String {
    base_url
        .and_then(|base| base.join(value).ok())
        .map_or_else(|| value.to_string(), |url| url.to_string())
}

fn is_descendant(document: &Html, child: NodeId, ancestor: NodeId) -> bool {
    document
        .tree
        .get(child)
        .is_some_and(|node| node.ancestors().any(|item| item.id() == ancestor))
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

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;

