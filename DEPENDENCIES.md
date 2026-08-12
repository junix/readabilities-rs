# Dependency and license audit

Audit date: 2026-08-11. Command:

```sh
cargo metadata --locked --format-version 1 --all-features
```

All resolved packages reported an SPDX license expression; none had a missing
license field. Resolved expressions are permissive or file-level weak-copyleft
(`MPL-2.0`) dependencies; no GPL/AGPL dependency was found.

`cargo audit -q` also completed successfully against the current RustSec
advisory database on 2026-08-11 (243 locked all-feature packages, zero known
vulnerabilities reported).

| Direct dependency | Resolved | License | Role |
|---|---:|---|---|
| `ego-tree` | 0.11.0 | ISC | mutable DOM node identity and traversal used by the native extractor |
| `ammonia` | 4.1.4 | MIT OR Apache-2.0 | mandatory final sanitizer |
| `htmd` | 0.5.5 | Apache-2.0 | post-extraction Markdown rendering |
| `markup5ever_rcdom` | 0.38.0 | MIT OR Apache-2.0 | semantic Markdown handlers for code, math, and footnotes |
| `scraper` | 0.26.0 | ISC | native DOM parsing, scoring, cleanup, and declarative site selectors |
| `reqwest` | 0.13.4 | MIT OR Apache-2.0 | bounded origin HTTP |
| `clap` | 4.6.6 | MIT OR Apache-2.0 | CLI |

`readabilities-rs` does not call another extraction product. The Defuddle
source is the behavioral reference for the native Rust algorithm; Defuddle and
Python remain black-box comparison participants in the independent suite and
are not runtime dependencies. No feature launches Chrome, Python, Node, or a
PaaS CLI. The local pure-Rust `html-to-markdown-rs` project was behaviorally
evaluated but is not a dependency in v0.1; see README “Known divergences.”

The native-only guarantee here is about document acquisition, extraction, and
rendering: no external process or non-Rust content engine is invoked. It is not
a claim that every transitive machine-code instruction was authored in Rust.
The optional/default `http` feature uses Rustls through `reqwest`; its selected
cryptographic provider includes `aws-lc-sys` C/assembly for TLS primitives.
That code does not parse, select, clean, or render document content. Builds with
`--no-default-features` omit the network/TLS stack entirely.
