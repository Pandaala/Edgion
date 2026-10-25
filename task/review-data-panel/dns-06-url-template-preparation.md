# Evaluate prepared HTTPDNS URL templates

Status: deferred for user decision. No runtime change.
Source baseline: `0b9881a62`.

## Evidence and decision

Common `dns/httpdns.rs::render_url` validates the query domain, selects a
nonempty explicit template or a preset, then replaces every literal `{domain}`.
The explicit-template branch first checks `contains`, so replacement searches
again after the first existence search. The selected source and placeholder
locations are configuration-stable; the domain and rendered output are not.

`HttpDnsCandidate::prepare` already owns a query-policy copy and prepared response
metadata in `PreparedHttpDns`. Ready queries still call `render_url` through
`resolve_http_dns_prepared`. Both Gateway native/AI consumers and Controller
`ServiceDnsState` use this Common prepared owner. A preparation failure instead
runs its configured fallback; it does not reach primary URL rendering.

Consider retaining the current standard-library replacement, preparing only the
selected-template state, or preparing placeholder byte ranges in the existing
immutable generation. Ranges can avoid repeated searches but add retained
metadata proportional to placeholder count; copied segments would additionally
retain template bytes. Neither representation removes the final owned URL or
its subsequent parsing by the fetcher. A selected-template state alone does not
remove replacement's placeholder search. Decide value from measured query costs
and retained-generation costs before selecting an implementation.

The inspected Rust 1.96.1 `alloc/src/str.rs::replace` already walks `match_indices`
into one output String; it does not build a segment vector or a temporary String
per placeholder. Its initial capacity depends on pattern/replacement lengths and
is not an exact expanded-output reservation. Any capacity proposal must account
for domain length, all occurrences and overflow without claiming that saturating
arithmetic makes allocation failure recoverable. No performance benefit has been
measured for the proposed preparation.

## Required acceptance

- Preserve domain-first validation and exact errors for empty/invalid domains,
  missing placeholders and absent template/preset. Production readiness checks
  do not remove the public `render_url` and `resolve_http_dns_with` contracts.
  Preparing metadata must not move observable query errors ahead of domain
  validation or change fallback/error precedence.
- Preserve explicit nonempty-template precedence, empty/absent-template preset
  fallback, and the independent preset response-format precedence. Replace all
  nonoverlapping literal occurrences, including adjacent/repeated placeholders;
  preserve all other bytes, braces and authored signed paths/queries.
- Keep runtime domain validation. Do not broaden the allowed hostname alphabet,
  decode/re-encode authored URLs, or substitute into a parsed placeholder URL
  without separately proving exact output and endpoint-policy equivalence.
  The fetcher must still classify normalized literals and send that same parsed
  URL through the canonical client, with existing URL-free SDK diagnostics,
  timeout, redirect, proxy and response-cap behavior.
- Own metadata in the existing immutable prepared generation. Keep old queries
  tied to their original source; do not add a global template cache, new generic
  template engine, lazy per-query preparation or a second publication authority.
  Preserve address/runtime identity and failed-generation fallback semantics.
  Do not duplicate connection or Secret material to prepare query metadata.
- Freeze an exact implementation allow-list after tracing direct helpers and
  Gateway/Controller prepared consumers. Keep pure helper tests and production
  generation tests distinct. Cover multiple/adjacent placeholders, unchanged
  non-placeholder bytes, preset overrides, invalid-input error ordering, failed
  preparation, and old/new generation independence. Existing rendering tests
  alone do not establish every prepared-publication boundary.
- Compare short and long templates, one/many occurrences and short/long valid
  domains. Separate scan time, output allocations and retained metadata across
  concurrent old/new generations; include cold preparation cost and the existing
  standard-library baseline. Do not add payload logging or query-time tracing.

Use IMPL after user selection. This is distinct from [Standard DNS preparation](dns-03-standard-resolver-preparation.md)
and [JSON response projection](dns-04-json-response-projection.md). Original
contracts: [prepared HTTPDNS](../../skills/04-review/link-sys/regression-guards/httpdns-prepared-runtime.md)
and [endpoint egress](../../skills/04-review/ssrf/regression-guards/httpdns-endpoint-egress.md).

Fresh existing Common DNS tests passed 71; formatting, workspace all-target
check/Clippy and agent-doc validation passed with existing warnings. No tests or
runtime code changed; no full suite, new transport experiment, prepared-template
prototype or allocation/latency benchmark ran.
