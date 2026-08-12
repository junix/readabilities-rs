# Known divergences from Crawlberg

These differences are intentional. Crawlberg is a source-level reference for
selected parsing and acquisition behaviors, not the product boundary or a
runtime dependency of `readabilities-rs`.

| ID | Domain | Crawlberg | readabilities-rs | Reason | Since |
|---|---|---|---|---|---|
| D1 | scope | Crawls sites, follows links, streams batches, and stores results | Extracts one supplied snapshot or one directly fetched URL | Preserve the small readability-engine contract | 2026-08-12 |
| D2 | browser | May render JavaScript and bypass WAFs with optional browser backends | Never launches a browser or external process | Static snapshot provenance and native-only runtime are contractual | 2026-08-12 |
| D3 | canonical-content | Converts the full fetched page to Markdown, then optionally prunes Markdown lines | Selects and sanitizes canonical clean HTML before any renderer runs | Output format must not change content selection | 2026-08-12 |
| D4 | markdown-engine | Uses `html-to-markdown-rs` and exposes fit Markdown, Djot, structure, tables, and citations | Uses `htmd` with custom code, math, image, and footnote handlers | The frozen corpus found structure regressions in the alternative engine; switch only after parity evidence | 2026-08-12 |
| D5 | discovery | Returns whole-page links, images, feeds, favicons, and raw JSON-LD | Returns an article plus normalized metadata; JSON-LD currently enriches that metadata | Avoid mixing page chrome inventories into the readable article model | 2026-08-12 |
| D6 | robots | Enforces robots.txt and reports noindex/nofollow for crawling | Does not crawl or enqueue discovered URLs | Robots policy belongs to traversal, which is outside this crate | 2026-08-12 |

## Matching rules

- Treat browser, WAF, crawl frontier, sitemap, robots traversal, proxy, storage,
  API, MCP, binding, and document-download changes as `INTENTIONAL_SKIP` unless
  the local public contract is explicitly expanded first.
- Re-review HTML parsing, charset detection, URL resolution, normalized metadata,
  SSRF, redirect, and byte/deadline fixes for small native ports.
- Re-review Markdown changes only against the frozen rich-structure corpus; do
  not infer parity from converter unit tests alone.
