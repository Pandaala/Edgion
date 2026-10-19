# Evaluate shared OIDC claims serialization

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `3da492335`.

## Evidence and decision

OpenidConnect::apply_upstream_headers in
`edgion-gateway/src/plugins/http/openid_connect/userinfo.rs` serializes the same
immutable claims Value twice when set_userinfo_header is true, userinfo_json is
None and store_claims_in_ctx is true. Request-filter authentication calls this
after fetch_userinfo_json; missing endpoints and best-effort fetch failures can
reach the claims fallback. Both output switches default to false.

Decide whether sharing one serialized String is worthwhile for this workload.
The existing review evidence rejected the narrow optimization on expected low
benefit, but contains no measurement establishing frequency, claims sizes or
execution cost. Reuse can remove one traversal and temporary output allocation;
it cannot remove the independent header and context storage. The real adapter's
set_ctx_var still copies the supplied text into request-owned context.

Currently the fallback String is dropped before Authorization is formatted;
reuse would retain it across that intervening write. Measure live storage as
well as CPU/allocation counts before describing the tradeoff. No candidate or
benchmark has been implemented, and no peak-memory regression is established.

## Boundaries and acceptance for later implementation

Preserve opt-in defaults and publication order: access token, ID token, userinfo,
Authorization, claims context, then compiled claim-header projection. Supplied
userinfo is a distinct document and must never replace claims context. Preserve
None versus Some(empty) behavior and fallback serialization only when needed.

Header value/aggregate caps, sensitive flags, unsafe-value rejection and setter
failure handling remain unchanged. A rejected or failed userinfo header write
must not suppress claims context. Context remains independent of header limits;
projection receives the same remaining budget. Keep serialization and context
setter failure behavior, canonical logging/redaction and best-effort fetching.
Do not add a JSON cache, typed ctx API, response aggregation or per-request logs.

If approved, use IMPL and test both flags independently and together, absent
versus supplied userinfo (including empty text), exact/oversized header budgets,
failed header writes and operation order. Measure representative claims sizes
with both Authorization settings. Keep identical JSON text and ownership after
return. Existing header tests cover supplied userinfo, oversized ID tokens,
unsafe userinfo and failed fixed-header budget accounting; they do not establish
shared-buffer equivalence or workload performance.

Related evidence:
`skills/04-review/cpu-memory/review-evidence/oidc-jwt-claims-json-serialization-opt-in.md`.
The separate condition-01 task concerns later parsing of context, not producer
serialization. This task changes no runtime behavior.
