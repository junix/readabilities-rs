# Three-participant conformance profile

The independent `readabilities-suite` compares these real public surfaces:

| Participant ID | Public surface | Local profile |
|---|---|---|
| `defuddle` | `defuddle parse ... --json --markdown` | Defuddle defaults |
| `readabilities-py` | one-shot Python driver calling public library APIs | local Readability/Trafilatura only |
| `readabilities-rs` | `readabilities-rs extract ... --format markdown` plus JSON metadata projection | `Balanced` |

Adapters may translate native output into the normalized record below. They
must not select a different DOM subtree, remove text, repair metadata, or call a
fourth readability implementation.

```json
{
  "schema_version": 1,
  "participant": "readabilities-rs",
  "status": "success | no_content | failure | unsupported",
  "content_format": "html | markdown | text",
  "content": "native result",
  "normalized_text": "whitespace-only normalization",
  "metadata": {"title": null, "author": null, "published": null, "site": null},
  "elapsed_ms": 0,
  "warnings": []
}
```

## Canonical invariant

For identical HTML bytes and base URL, every mandatory participant returns a
normalizable primary-content result through its real public interface. The
result retains every `required_text` fact, excludes every `forbidden_text`
fact, preserves required structure, and reports success/no-content/failure
without adapter inference.

## Oracle fields

Each pinned case declares:

- stable case ID `READ-NNN`;
- fixture provenance and base URL;
- `required_text` and `forbidden_text` facts;
- `required_structure` such as heading, code, table, image, footnote, or math;
- exact `required_markdown`/`forbidden_markdown` facts and generic Markdown
  syntax invariants;
- required fact ordering;
- expected metadata fields;
- expected Rust site-configuration provenance, including the no-match fallback;
- mandatory security facts;
- participant capabilities required by the profile.

Quality dimensions remain separate:

1. required-text recall;
2. forbidden-noise rejection;
3. structure preservation;
4. Markdown fidelity and fact ordering;
5. metadata and site-configuration accuracy;
6. security violations;
7. execution cost and wall time.

No aggregate score may hide a security failure or a required-text regression.
The Rust implementation may be called best on the frozen corpus only when it
has zero failed cases and security violations and is no worse than the best
participant on recall, noise rejection, structure, Markdown fidelity, fact
ordering, and metadata. Runtime cannot repair a quality failure. Otherwise
reports list the cases won by each implementation.

## Profiles

- `offline` (default gate): committed, redistributable fixtures; no network;
  every mandatory participant must run and cannot become green through SKIP.
- `live`: fetches current public pages once, records capture metadata, and
  compares the resulting common snapshot. Discovery only, never a release gate.

Managed extraction-provider profiles are intentionally outside the Rust
contract. External engines may be comparison participants, but never a runtime
dependency or fallback of `readabilities-rs`.

## Failure proof

Before accepting the suite, one participant is deliberately replaced with a
fixture that omits a required fact. The suite must fail with the case ID,
participant, expected fact, actual bounded preview, and non-zero exit status.
The participant is then restored and the same case must pass.
