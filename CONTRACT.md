# readabilities-rs public contract

This document freezes the observable v1 contract. Internal extraction rules
and DOM libraries may change without changing this contract.

## Purpose

`readabilities-rs` finds the primary readable content in an HTML page, removes
page chrome and unsafe markup, preserves useful document structure, and only
then derives Markdown or plain text. It is not an HTML-to-Markdown converter.

## Library surface

```rust
let reader = readabilities_rs::Reader::new();

// Pure, synchronous, and network-free.
let article = reader.extract_html(html, base_url.as_ref())?;

// Origin HTTP plus the local extractor by default.
let article = reader.read_url(&url, &UrlPolicy::default()).await?;

// Detailed, non-throwing execution evidence.
let execution = reader.execute(ReadRequest::html(html, base_url)).await;

// Rendering never re-runs extraction.
let markdown = article.render(OutputFormat::Markdown)?;
```

The convenience methods are projections of `Reader::execute`; they do not
implement a second pipeline.

## Invariants

1. `extract_html` never performs network I/O, even for site-specific content.
2. `UrlPolicy::default()` fetches the supplied origin and runs local Rust
   extraction. Managed providers and external-process acquisition are not part
   of the dependencies, library, or CLI.
3. Every successful `Article` contains non-empty, parseable, finally sanitized
   HTML. Script elements, event-handler attributes, and dangerous URI schemes
   are never returned.
4. Clean HTML is canonical. Markdown and text are derived from that same HTML;
   selecting an output format cannot affect content selection.
5. No-content, origin failure, and budget exhaustion remain distinct typed
   failures. They are never converted to an empty successful string.
6. `Execution` records attempts, stage timing, warnings, provenance, bounded
   removal previews, and acquisition resource counts without logging secrets.
7. Static HTML never claims observations such as computed styles, element
   geometry, executed JavaScript, or flattened shadow DOM.

## Input modes

- `Source::Html`: caller-provided HTML bytes plus an optional base URL.
- `Source::Url`: an absolute HTTP(S) URL and an explicit `UrlPolicy`.

Input validation rejects an empty HTML document, non-HTTP(S) URL acquisition,
embedded URL credentials, invalid budgets, and incompatible backend policies.

## Extraction modes

- `Balanced` (default): Defuddle-derived deterministic cleanup with a
  conservative short-result retry.
- `Conservative`: favors recall and retains uncertain blocks.
- `Aggressive`: favors noise rejection and may remove more low-scoring blocks.
- `Ensemble`: runs Balanced, Conservative, and Aggressive variants of the
  local Rust pipeline, then selects the highest quality score with earlier
  attempts winning ties. It is unavailable without the `ensemble` Cargo
  feature.

The generic extractor remains capable without a site match. A `SiteConfig` may
declare host/path matching, one content-root selector, and removal selectors.
Caller configurations precede built-ins; matching is boundary-aware; every
configuration is validated before extraction. Site configuration is a narrow
layout hint, not an alternate pipeline: generic cleanup, normalization, and the
final sanitizer still run. Medium, Wikipedia, and MDN are built in. Arbitrary
rule-weight and sanitizer overrides are intentionally not public in v1.

## Quality semantics

`QualityBand::{Strong, Usable, Weak}` is an evidence band, not a probability.
A weak article is still sanitized and non-empty. A retry or secondary engine
that wins marks the result `Degraded` and adds an attempt record; callers can
reject degraded output in policy.

Release quality is evaluated on actual Markdown, not only extraction speed or
clean-HTML shape. Required content, forbidden noise, structure, Markdown
syntax/semantics, fact ordering, metadata, site selection, and security remain
separate facts. Runtime is reported only as a secondary dimension and cannot
offset a failed quality fact.

## Output and provenance

`Article` includes canonical HTML, metadata, word count, quality, warnings,
and provenance. `Article::render` supports `Html`, `Markdown`, `Text`, and
`Json`. Markdown is always derived from the same clean HTML.

## CLI surface

```text
readabilities-rs extract [PATH|-] [--url BASE] [--format json|html|markdown|text]
  [--site-config PATH]... [--no-site-configs]
readabilities-rs read URL [--format ...]
  [--site-config PATH]... [--no-site-configs]
readabilities-rs doctor [--json]
readabilities-rs version [--json]
```

`extract` defaults to stdin and is hermetic. `read` is the only command that
performs acquisition. Diagnostics go to stderr; requested content goes to
stdout. JSON output is stable enough for the comparison suite and contains a
`schema_version` field.

## Compatibility

Semver applies to exported Rust names, CLI commands/options, JSON field meaning,
error categories, and the invariants above. Exact serialized HTML bytes,
removal counts, stage timings, and heuristic scores are not stable API.
