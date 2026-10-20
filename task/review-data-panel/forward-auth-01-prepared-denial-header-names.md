# Prepared ForwardAuth denial header names

Status: deferred ownership and memory tradeoff for user decision. No runtime change.
Source baseline: c7edfde54.

## Evidence and decision

In `edgion-gateway/src/plugins/http/forward_auth/plugin.rs`, from_client already
prepares first-equivalent client_headers selection, preserving first spelling
and configuration order. send_error_response still clones the selected String
for each retained source value passed to insert_header or append_header.
Names with no retained value incur no such payload copy. This is separate from
the completed configuration-name deduplication optimization.

The locked Edgion Pingora revision 1aaa45cc70430bc782990bc20ddee3fef36d6b99
accepts String, Bytes and CaseHeaderName. CaseHeaderName owns Bytes and preserves
original spelling; an arbitrary borrowed str cannot satisfy its static-string
conversion. Passing a parsed HeaderName instead changes spelling through Pingora's
standard-name title mapping or normalized lowercase representation. Both insertion
helpers still parse original-case bytes into HeaderName, even with CaseHeaderName.
A prepared case-preserving owner could remove repeated name payload copies, but
would not remove normalized-name parsing or response-map allocations.

Evaluate a generation-owned prepared name representation against retaining the
current Strings. Shared names add ownership metadata and potentially another
buffer beside resource configuration. First-clone promotion, warm clones and
live old-generation responses must be measured separately. Denial frequency,
selected-name lengths and repeated values determine whether this is worthwhile;
no net heap or throughput benefit is established. Keep Pingora types in Gateway
runtime ownership, outside resource schema and conf_sync transport. Reuse existing
canonical types; do not introduce a global cache or request-time preparation.

This resembles the ownership tradeoff in
[session-persistence header names](lb-04-prepared-session-header-name.md), but
has separate denial, repetition and protection semantics. It is not a request
to redesign Pingora's header API or every header producer.

## Acceptance for a selected implementation

Preserve original case, first-equivalent configured-name order and every valid
repeated source value. Keep first-successful-insert then append behavior; a failed
write must retain its diagnostic and must not advance that state. Keep invalid-text
filtering and shared protected-field policy. Every received Connection line must
participate in nomination filtering; opaque Connection suppresses remote copies,
including automatic Content-Type. Do not cache response-dependent nominations.

Preserve body ownership and its existing streaming cap on the separate outbound
exchange, denial status classification, authentication delay, automatic Content-Type,
transport-failure JSON, queued outcome ordering, EOS and shutdown. Success-side
upstream_headers, source config and admission validation remain unchanged.

Measure current versus prepared first/warm insertion and repeated append with
case maps enabled and disabled, standard/custom/mixed-case names, absent fields,
invalid values and varying name lengths. Account for retained heap during config
reload and overlapping old requests, not only allocation count inside the writer.
Cover both Route-owned and EdgionPlugins-owned construction. Existing ForwardAuth
denial tests are a starting point; add direct original-case evidence and meaningful
allocation/ownership checks for the selected representation. Integration fixtures
remain frozen in this campaign. Obtain independent IMPL plan/final review and
project checks before implementing a chosen alternative.

Original knowledge:
- skills/04-review/forward-auth/regression-guards/deny-response-repeated-headers-preserved.md
- skills/04-review/forward-auth/regression-guards/deny-response-content-type-preserved.md
- skills/04-review/forward-auth/regression-guards/send-error-response-header-insert-plugin-log.md
- skills/04-review/plugin-runtime/regression-guards/response-header-mutation-policy.md

Fresh baseline: all 13 Gateway ForwardAuth tests passed serially, plus formatting
and workspace all-target check/Clippy. Existing unrelated Clippy warnings remain.
These checks establish current behavior, not candidate performance or live network
coverage. No runtime code or tests changed.
