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
| Managed extraction providers | unsupported | unsupported | no | absent from API and CLI |
| Python/Node subprocess extraction | unsupported | unsupported | no | rejected policy |
| Browser/PaaS CLI execution | unsupported | unsupported | no | absent from dependencies, API, and CLI |
| Hidden remote fallback | unsupported | unsupported | no | validation error |

## Snapshot observations

Static caller HTML and origin HTTP response bytes are the only snapshot kinds.
The crate does not execute JavaScript, inspect computed styles or geometry, or
flatten shadow DOM. Callers that need those observations must provide captured
HTML themselves; `readabilities-rs` will still perform extraction locally.

## Cargo feature contract

- `--no-default-features`: synchronous local HTML extraction, sanitizer, and
  all renderers compile and test without an async runtime or network client.
- default: `http` plus `ensemble`.
- `all-features`: native extraction plus direct origin HTTP; there is no
  external-process or managed-provider feature.
