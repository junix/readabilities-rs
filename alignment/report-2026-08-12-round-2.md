# Crawlberg alignment report — 2026-08-12, round 2

Compared snapshots:

- `readabilities-rs`: working tree based on
  `283c1045cfb957ed6f8c4fc53cb11e2456d36317`
- `/Users/junix/crawlberg`: `e855adfa5a22bef61ba747f3b56ad4fb27fe4b5b`

Crawlberg has not advanced past the baseline recorded in
`alignment/config.yaml`. This round therefore re-audited the existing upstream
history and replayed Crawlberg's real HTML fixtures through the committed
`readabilities-rs` binary. It found one security omission hidden by the first
round's broad capability classification. The two actionable parsing and
security gaps were then ported natively in this working tree.

## Summary

| Classification | Count |
|---|---:|
| Ported now | 2 |
| Already aligned | 7 |
| Review behind an explicit contract | 3 |
| Keep as intentional divergence | 6 |

## Ported now

### Canonicalize IPv4-embedded IPv6 before SSRF classification

Crawlberg commit `c01188617` fixed a real SSRF bypass by converting these forms
to IPv4 before applying the private-network deny rules:

- IPv4-mapped IPv6, such as `::ffff:127.0.0.1` and
  `::ffff:169.254.169.254`
- RFC 6052 well-known NAT64 addresses under `64:ff9b::/96`

Before this port, `readabilities-rs::http::is_non_public` classified those
values only as ordinary IPv6. They did not match its loopback, link-local,
unique-local, multicast, unspecified, or documentation checks, even though a
dual-stack host can route them to the embedded IPv4 destination.

The native port now canonicalizes with `Ipv6Addr::to_ipv4_mapped`, explicitly
recognizes `64:ff9b::/96`, and reuses the existing IPv4 classifier. Targeted
tests cover mapped loopback, link-local metadata, RFC 1918 addresses, NAT64
loopback, mapped public IPv4, and genuine public IPv6.

The public-path regression test also exposed a second part of the same bug:
`Url::host_str()` serializes an IPv6 literal with brackets, so the old parser
failed to recognize it as an IP and sent it through DNS. Network validation and
client pinning now branch on the typed `url::Host`, ensuring IPv4 and IPv6
literals never go through DNS.

This finding supersedes the first report's blanket statement that origin SSRF
safety was already fully aligned. DNS validation, address pinning, no-proxy
requests, redirect validation, deadlines, and streamed byte bounds remain
aligned; the embedded-address classification and typed-literal entry path are
now closed.

### Match metadata names case-insensitively

Crawlberg lowercases each `<meta name>` or `<meta property>` value before
dispatching it. Before this port, `readabilities-rs` used exact CSS selectors such as
`meta[name="dc.title"]`.

Replaying Crawlberg's `dublin_core.html`, which uses conventional names like
`DC.title` and `DC.creator`, produced only the generic document title and HTML
language in the normalized result. The Dublin Core title, creator, description,
date, identifier, publisher, language, and subjects were missed.

The implementation now traverses `<meta>` elements once, normalizes `name` and
`property` with ASCII case folding, and feeds the existing normalized `Metadata`
fields while preserving source-type priority. This fixes Dublin Core and makes
OG/Twitter lookup robust without adding Crawlberg's larger metadata model.

## Fixture replay

| Crawlberg fixture | Result in current Rust binary | Assessment |
|---|---|---|
| `base_tag_page.html` | All relative links used the document base | Aligned |
| `protocol_relative_links.html` | URLs inherited the source scheme | Aligned |
| `comprehensive_metadata.html` | Title, author, description, dates, site, image, canonical URL, and keywords were retained | Aligned |
| `og_metadata.html` | Normalized OG/article fields and repeated tags were retained | Aligned |
| `twitter_card.html` | Twitter title, creator, description, and image were retained | Aligned |
| `dublin_core.html` | Uppercase `DC.*` title, author, description, date, publisher, identifier, language, and subjects were retained | Aligned after port |
| `malformed.html` | Returned typed `no_content` without crashing | Different product boundary; review only |

The malformed fixture omits `</title>`, so the HTML5 parser consumes the later
markup as title text. Crawlberg retains the raw fetched HTML and uses a regex
fallback for a small metadata subset; its fixture only requires non-empty raw
HTML plus the description. `readabilities-rs` promises selected readable
content instead of returning the raw response, so the two success conditions
are not equivalent.

## Already aligned

1. HTTP charset, BOM, and early meta-charset decoding.
2. Streamed response collection with a post-decompression byte ceiling.
3. `<base href>`, ordinary relative URLs, and protocol-relative URLs.
4. JSON-LD arrays, `@graph`, and article-node preference.
5. Common head metadata, OG, Twitter Card, and article tags.
6. Full removal of `noscript` content during DOM cleanup.
7. DNS validation and pinning, no-proxy requests, strict redirect handling,
   deadlines, and request/download budgets, including embedded-IPv4 handling.

## Review behind an explicit contract

1. **Malformed-head metadata recovery** — a bounded raw-head scanner could
   salvage description, OG, and Twitter metadata when DOM parsing loses the
   head. Do not copy a general HTML regex blindly: it can observe false `<meta>`
   text in comments, scripts, or attacker-controlled content.
2. **HTML/media-type validation** — retain the first report's recommendation to
   reject obvious binary or document bodies while tolerating absent and commonly
   mislabelled `Content-Type` values.
3. **Scoped private-network authorization** — exact host and CIDR grants would
   be safer for local services than the current all-or-nothing
   `allow_private_networks`. Preserve this crate's DNS validation and address
   pinning; do not copy Crawlberg's hostname allowlist short-circuit, which can
   bypass resolution before policy evaluation.

## Intentional divergences

The six entries in `alignment/known-divergences.md` remain valid: site crawling,
browser/WAF execution, whole-page resource discovery, robots traversal,
full-page Markdown pruning, and replacing the proven Markdown engine should not
be ported into the current article-reader contract.

## Verification

- Focused SSRF tests prove mapped/NAT64 private denial, mapped/public IPv6
  allowance, and typed IPv6-literal handling without DNS.
- A focused native test covers mixed-case Dublin Core and OG names while the
  real Crawlberg `dublin_core.html` fixture now produces all normalized fields.
- `just check-all` passes default, no-default, and all-feature tests/builds plus
  formatting and Clippy.
- The sibling offline suite keeps Rust at 17/17 cases, 0 security findings, and
  eligible status.

Malformed-head recovery, media-type validation, and scoped private allowlists
remain separate API/security decisions rather than implicit ports.
