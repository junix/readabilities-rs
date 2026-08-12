# readabilities-rs

`readabilities-rs` is a local-first readable-content engine. Its primary job is
to identify the main document, remove navigation/ads/related-content noise,
preserve meaningful structure, and enforce a final HTML security boundary.
Markdown and text are renderings of that cleaned article; they are not used to
decide what the article is.

Every build uses the native Rust pipeline and does not require Python, Node,
Chrome, a PaaS CLI, or another executable. HTML input never accesses the
network. URL acquisition talks directly to the requested origin over HTTP; the
crate never submits a URL or page content to a managed extraction provider.

The native extraction pipeline uses the Defuddle source tree as its behavioral
reference, but owns its Rust implementation and runtime. Defuddle, Python, and
other engines are comparison oracles only.

## Quick start

```sh
cargo build --locked

# Local file, canonical Article JSON
readabilities-rs extract page.html --url https://example.com/article --json

# stdin, rendered after extraction
curl -sS https://example.com/article |
  readabilities-rs extract - --format markdown

# Origin fetch with local extraction
readabilities-rs read https://example.com/article --format html

# Bounded attempts, removals, stages, provenance, and cost evidence
readabilities-rs extract page.html --debug
```

Install the all-feature release binary to
`~/sync/<os>-<arch>-bin/readabilities-rs`:

```sh
just install
```

## Library

```rust
use readabilities_rs::{OutputFormat, Reader};
use url::Url;

let html = r#"<nav>Noise</nav><article><h1>Title</h1><p>Body.</p></article>"#;
let base = Url::parse("https://example.com/post")?;
let article = Reader::new().extract_html(html, Some(&base))?;

assert!(article.content.contains("Body."));
let markdown = article.render(OutputFormat::Markdown)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Advanced callers use `ReadRequest` and `Reader::execute` to receive the single
structured execution model. `UrlPolicy::fallbacks` is ordered and explicit;
the only supported acquisition backend is direct origin HTTP, and there is no
third-party reader route.

## Site configurations

The generic extractor is always the default fallback. For layouts with stable,
known chrome, a declarative site configuration can narrow the content root and
remove selectors before the same generic cleanup, normalization, and mandatory
sanitizer run. Built-in configurations currently cover Medium, Wikipedia, and
MDN; matching is host-boundary and path-prefix aware.

```sh
# Built-ins are enabled by default.
readabilities-rs read https://en.wikipedia.org/wiki/Readability --format markdown

# Caller configuration takes precedence over built-ins; the option is repeatable.
readabilities-rs extract page.html --url https://docs.example.test/guide \
  --site-config sites.json --format markdown

# Diagnose the portable generic fallback explicitly.
readabilities-rs extract page.html --url https://example.test/post \
  --no-site-configs --format markdown
```

```json
{
  "id": "docs-example",
  "hosts": ["docs.example.test"],
  "path_prefixes": ["/guide/"],
  "content_selector": "main",
  "remove_selectors": [".sidebar", ".feedback", "footer"]
}
```

The library exposes `SiteConfig`, `parse_site_configs`,
`Reader::with_options_and_site_configs`, and
`Reader::without_site_configs`. A configuration chooses and cleans a known
layout; it cannot bypass the general engine or the security boundary.

## Backend routing

| Need | Route | Network/data consequence |
|---|---|---|
| Already have HTML | `extract_html` / `extract` | Offline |
| Ordinary server-rendered URL | origin `read_url` / `read` | Origin only |
| Page requires JavaScript execution | unsupported | supply already-rendered HTML explicitly |
| Native strategy comparison | `ExtractionMode::Ensemble` | Offline Balanced, Conservative, and Aggressive Rust candidates |
| Managed reader services | unsupported | Rust never submits URLs or content to extraction vendors |
| External browser/Python/Node/PaaS CLI | unsupported | no subprocess launch path exists |

Origin HTTP enforces an overall deadline, byte/request/redirect budgets,
cross-origin redirect denial by default, and SSRF-resistant DNS validation and
address pinning. Private, loopback, link-local, multicast, and documentation
networks are denied unless the caller explicitly allows them.

Every acquisition path converges on the same local extraction and mandatory
Ammonia sanitizer.

## Cargo features

| Feature | Default | Capability |
|---|---:|---|
| `http` | yes | bounded origin acquisition and async execution |
| `ensemble` | yes | compare native Balanced, Conservative, and Aggressive candidates |

The local extractor and CLI still compile and test with
`--no-default-features`.

## What “best” means here

The independent sibling project `readabilities-suite` invokes Defuddle,
`readabilities-py`, and this binary through their real public surfaces and
compares their actual Markdown. On the frozen 16-case corpus from 2026-08-11,
Rust passed 16/16 with required-text recall 43/43, noise rejection 38/38,
structure preservation 33/33, Markdown fidelity 70/70, fact ordering 14/14,
metadata 19/19, and zero security violations. The report is in
`../readabilities-suite/reports/offline.md`.

That is a bounded corpus claim, not a claim of universal superiority. The live
8-page common-site report is intentionally non-gating because public pages
drift and have no frozen fact oracle. It records successful extraction,
configuration matches, Markdown statistics, and security signals without
calling shorter or faster output better.

## Known divergences

- Full Defuddle site-extractor parity is not claimed. Medium, Wikipedia, and
  MDN have declarative built-ins; external configurations and the no-match
  generic fallback have dedicated E2E fixtures. New site rules require a
  failing, self-authored fixture and may not compensate for a weaker default.
- List pages such as Hacker News may produce more content than article-focused
  extractors. Live reports expose the size difference; they do not call shorter
  output better without a content oracle.
- The local pure-Rust `html-to-markdown-rs` library was evaluated on the rich
  fixture with its table plugin enabled. It currently removes `<sup>` footnote
  structure, loses whitespace after an image, and emits an empty unsafe link;
  `htmd` preserves the gated Markdown facts after the mandatory sanitizer, so
  v0.1 continues to use `htmd` directly as a Rust library.

## Development

```sh
just check-all       # fmt, strict clippy, default/no-default/all-feature tests and builds
just benchmark 100   # speed/memory evidence after the quality gate passes
```

The public contract and feature matrix live in [CONTRACT.md](CONTRACT.md),
[CAPABILITIES.md](CAPABILITIES.md), and [CONFORMANCE.md](CONFORMANCE.md).
Dependency and benchmark evidence live in [DEPENDENCIES.md](DEPENDENCIES.md)
and [BENCHMARK.md](BENCHMARK.md).

## License

MIT. See [LICENSE](LICENSE).
