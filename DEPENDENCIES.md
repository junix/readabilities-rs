# Dependency and license audit

Audit date: 2026-08-11. Command:

```sh
cargo metadata --locked --format-version 1 --all-features
```

All resolved packages reported an SPDX license expression; none had a missing
license field. Resolved expressions are permissive or file-level weak-copyleft
(`MPL-2.0`) dependencies; no GPL/AGPL dependency was found.

`cargo audit -q` also completed successfully against the current RustSec
advisory database on 2026-08-11 (319 locked dependencies, zero known
vulnerabilities reported).

| Direct dependency | Resolved | License | Role |
|---|---:|---|---|
| `decruft` | 0.2.0 | MIT | main-content location, cleanup, normalization, site-aware extraction |
| `trafilatura` | 0.3.0 | Apache-2.0 | optional independent local candidate |
| `ammonia` | 4.1.4 | MIT OR Apache-2.0 | mandatory final sanitizer |
| `htmd` | 0.5.5 | Apache-2.0 | post-extraction Markdown rendering |
| `markup5ever_rcdom` | 0.38.0 | MIT OR Apache-2.0 | semantic Markdown handlers for code, math, and footnotes |
| `scraper` | 0.26.0 | MIT OR Apache-2.0 | validated declarative site selectors |
| `reqwest` | 0.13.4 | MIT OR Apache-2.0 | bounded origin/provider HTTP |
| `chromiumoxide` | 0.9.1 | MIT OR Apache-2.0 | opt-in browser acquisition |
| `pulldown-cmark` | 0.13.4 | MIT | managed Markdown to local HTML boundary |
| `secrecy` | 0.10.3 | Apache-2.0 OR MIT | provider credential handling |
| `clap` | 4.6.6 | MIT OR Apache-2.0 | CLI |

`readabilities-rs` does not copy Python implementation code. Defuddle and
Python are black-box comparison participants in the independent suite. The
local `html-to-markdown-rs` project was behaviorally evaluated but is not a
dependency in v0.1; see README “Known divergences.”
