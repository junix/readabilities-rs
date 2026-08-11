# Quality and performance snapshot

## Quality performance: release gate

Command: `readabilities-suite run --profile offline`
Corpus: 16 self-authored cases, including Medium, Wikipedia, MDN, an external
site configuration, and an unconfigured generic fallback

| Participant | Cases | Required | Noise | Structure | Markdown | Order | Metadata | Security |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Defuddle | 4/16 | 43/43 | 34/38 | 23/33 | 67/70 | 14/14 | 19/19 | 1 |
| readabilities-py | 3/16 | 43/43 | 32/38 | 22/33 | 60/70 | 14/14 | 19/19 | 2 |
| readabilities-rs | **16/16** | **43/43** | **38/38** | **33/33** | **70/70** | **14/14** | **19/19** | **0** |

This is the primary performance claim: on the frozen corpus, Rust retains all
required content, removes all declared noise, and preserves the tested Markdown
semantics. It is a bounded corpus result, not universal superiority. Output
length is diagnostic only; less text may mean better cleanup or lost content.

## Runtime and memory: secondary evidence

Date: 2026-08-11
Machine: local Apple Silicon macOS checkout
Command: `just benchmark 100`
Corpus: 10 self-authored HTML fixtures, five alternating-order samples of 100
iterations (1,000 documents/sample); table values are medians

| Mode | Wall time | Average/document | Average output | Removal records |
|---|---:|---:|---:|---:|
| normal | 1,498.1 ms | 1,498.1 µs | 566 bytes | 0 |
| diagnostics | 1,501.9 ms | 1,501.9 µs | 566 bytes | 3,800 |

- Diagnostics median wall overhead: 0.25% in this run. Sample order alternates
  to reduce systematic warm-cache bias.
- Recorded parse/locate/clean/normalize stage totals in the median-wall samples:
  642 ms normal and 641 ms diagnostics. These internal integer timings are
  diagnostic evidence, not a replacement for the process wall clock.
  Normal wall samples ranged from 1,483.1–1,517.5 ms and diagnostic samples
  from 1,496.8–1,550.4 ms.
- Sub-millisecond validate/sanitize totals round to zero in the
  public millisecond records; wall time remains the authoritative aggregate.
- OS-reported maximum resident set size: 21,807,104 bytes.
- OS-reported peak memory footprint: 14,107,128 bytes.

This is a reproducible local baseline, not a cross-machine performance claim.
`just benchmark` builds the release example first, runs alternating normal and
diagnostic samples in one process, emits JSON, and wraps it with
`/usr/bin/time` for peak resident-memory evidence. These measurements are
reported only after the Markdown quality gate; they cannot compensate for
missing content, retained noise, or unsafe output.
