# Evaluate preserving validated passthrough SNI across route selection

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Inspected baseline: `35074f81a`.

## Evidence and decision

`routes/tls/sni_parser.rs::extract_sni` validates nested ClientHello containers,
decodes the selected name and calls the canonical normalize_route_sni from
`edgion-algo/src/matcher/radix_hostname/input.rs`. NormalizedRouteSni::query
can expose a validated query without repeating validation, but into_owned returns
an ordinary String. Passthrough stores that String in TlsContext.sni.

The shared select_tls_route helper passes the string to
`routes/tls/gateway_tls_routes.rs::TlsRouteTable::select_by`, which calls
normalize_route_sni again. For successful passthrough input this repeats the
uppercase scan and hostname validation, including IP-literal rejection and DNS
label checks. The accepted name is at most 253 bytes and already lowercase;
this second call does not allocate another lowercase string.

Attachment checks are a different operation: radix_hostname_matches is a pure
exact/suffix comparison without parsing or allocation. Do not justify compiling
another attachment hostname list by describing this predicate as a parser.
Current attachment indices borrow the immutable schema lists.

The decision is whether measured connection-path savings justify preserving a
checked owned name through a narrow typed handoff, or retaining the bounded
second validation. Compare concrete API and lifetime designs before choosing.
An additional context field, duplicate name storage or persistent cache is not
assumed necessary or authorized. Never add an unchecked raw-string flag to
bypass validation. Reuse the canonical Route hostname types and matcher.

Terminated TLS shares this selection path but obtains ctx.sni from handshake
metadata or SSL fallback extraction. Those inputs must retain their existing
validation boundary; a proof for passthrough does not cover every caller.
Connection logs also use ctx.sni, so changing its storage must preserve existing
spelling, serialization and lifetime. No performance improvement is measured.

## Acceptance for a later design

- Preserve missing, malformed, invalid UTF-8, uppercase, IP-literal, wildcard,
  trailing-dot and length-limit behavior for both TLS modes. Keep full parser
  container validation and first-SNI selection before publication.
- Keep raw/log-visible spelling separate from any normalized lookup semantics
  where the current mode distinguishes them. Avoid new diagnostics or metrics.
- Preserve exact/wildcard precedence, same-attachment hostname and logical-owner
  eligibility, more-specific listener authority, accepted-parent order and
  protocol checks. A selected backend/policy failure must not restart routing.
- Preserve immutable publication selection, old connection generations, and
  release of unselected tables and inspection bytes before long-lived relay.
  A borrowed query must never outlive its checked owner.
- Compare current and proposed scan costs, allocations and context/future storage
  for common and maximum-length names, lowercase and mixed-case input, hits and
  misses, and both modes. Measure end-to-end selection as well as normalization;
  do not convert a microbenchmark into a Gateway throughput claim.
- Use an independently reviewed exact implementation scope, canonical matcher
  tests, Gateway parser/table/proxy regressions and project checks. Revalidate
  socket-level ClientHello coverage if input ownership or transport flow changes.

Original contracts: [ClientHello parsing](../../skills/04-review/tls/regression-guards/sni-passthrough-clienthello-read.md),
[attachment eligibility](../../skills/04-review/routing/regression-guards/tls-hostnames-per-attachment.md),
and [passthrough ownership](../../skills/04-review/routing/regression-guards/tls-passthrough-connection-ownership.md).
This proposal is unimplemented and awaits user decision.
