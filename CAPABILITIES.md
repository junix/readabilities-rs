# Capability matrix

Legend: `required` is part of the default gate, `optional` needs a feature or
explicit policy, and `unsupported` is rejected rather than silently emulated.

| Capability | Library | CLI | Default | Evidence |
|---|---:|---:|---:|---|
| Local HTML extraction | yes | `extract` | required | article + attempts |
| Main-content selection | yes | yes | required | engine, quality signals |
| Noise removal | yes | yes | required | bounded removal records |
| Generic no-config fallback | yes | yes | required | unconfigured E2E case |
| Built-in site configuration | Medium/Wikipedia/MDN | automatic | required when matched | provenance + site E2E facts |
| External declarative site configuration | yes | `--site-config` | optional | validated selector/config case |
| Metadata/schema.org | yes | yes | required | typed metadata |
| Final HTML sanitizer | yes | yes | required, cannot disable | security facts |
| Clean HTML | yes | yes | required | canonical article HTML |
| Derived Markdown/text | yes | yes | required | native Markdown E2E facts + render provenance |
| Origin HTTP | `http` feature | `read` | required in default build | redirects/bytes/timing |
| Native ensemble | `ensemble` feature | `--mode ensemble` | optional | candidate comparison |
| Browser live DOM | `browser` feature | `--browser` | optional | snapshot observations |
| Jina Reader | `providers` feature | `--provider jina` | optional, explicit | requests/cost unknown |
| Firecrawl | `providers` feature | `--provider firecrawl` | optional, explicit | request/task IDs |
| YXT | `providers` feature | `--provider yxt` | optional, explicit | request/task IDs |
| Python/Node subprocess extraction | unsupported | unsupported | no | rejected policy |
| Hidden remote fallback | unsupported | unsupported | no | validation error |
| Streaming provider API | unsupported in v1 | unsupported | no | rejected policy |

## Snapshot observations

| Observation | Static/origin HTML | Browser snapshot |
|---|---:|---:|
| HTML bytes / serialized DOM | yes | yes |
| JavaScript executed | no | yes |
| Computed styles | no | not claimed in v1 |
| Element geometry | no | not claimed in v1 |
| Shadow DOM flattened | no | not claimed in v1 |

The browser capability intentionally reports only what its acquisition path
actually captured.

## Cargo feature contract

- `--no-default-features`: synchronous local HTML extraction, sanitizer, and
  all renderers compile and test without an async runtime or network client.
- default: `http` plus `ensemble`.
- `providers`: typed managed-reader adapters; implies `http`.
- `browser`: Chrome DevTools acquisition; implies `http`.
- `all-features`: every optional adapter compiles, but live tests still require
  explicit environment variables and never gate the offline suite.
