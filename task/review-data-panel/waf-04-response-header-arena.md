# Evaluate consolidated WAF response-head storage

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Inspected baseline: `28a3485a0`.

## Evidence and decision

The Gateway WAF response-header callback in
`edgion-gateway/src/plugins/http/waf/mod.rs` collects borrowed ResponseHeader views
from the live session map. It does not copy each header's bytes. The transaction
outlives that callback, so `publish_response_head` in
`edgion-waf/src/seclang/transaction.rs` builds independent owned ResponseState.
Borrowing the live map beyond publication is not a valid replacement.

ResponseState::build validates the complete head, reserves the entries vector,
copies decimal status bytes, then copies each header name and value into separate
Vecs through fallible owned_copy. Empty values need no payload allocation.
SelectedBytes::Response and SelectedName::Response resolve indexed entries;
CandidateCursor preserves entry order and collection identity.

A single immutable byte arena with range records could reduce allocation count
for nonempty fields. It still needs to copy bytes and retain ordered metadata.
Actual layout, allocation count, capacity, peak heap and callback latency have
not been measured. No specific byte saving or throughput benefit is established.

Current retained accounting charges payload lengths plus
entries.len() * size_of::<ResponseEntry>(), not Vec capacities or complete heap
usage. publish_response_head constructs the owned state before aggregate
max_retained_bytes admission and publishes it only on success. Changing record
layout can change near-limit admission; consolidation changes allocation failure
opportunities. Moving aggregate admission ahead of construction would also change
error precedence and must not be bundled silently.

The decision is whether measured benefits justify an arena representation, and
whether its accounting should preserve the current logical charge or adopt a
reviewed new descriptor charge. The request-head arena's nominal accounting in
`skills/04-review/waf/regression-guards/fused-json-inspection.md` is a scoped
precedent, not automatic permission to change response accounting or preserve an
unexplained historical size constant.

## Acceptance for later implementation

- Keep immutable ownership after input buffers are dropped or mutated. Preserve
  status bytes for every accepted status, duplicate header order, case handling,
  empty values and permitted binary bytes.
- Preserve response collection selection/counts, transformations, captures, TX
  effects and failed-chain rollback in Accelerated and Reference execution.
  Validate every range and retain the existing invariant-failure behavior.
- Preserve status/header validation order, content-free errors, phase guards,
  repeated-publication rejection and failed publication leaving state absent.
  Explicitly review allocation failure and arithmetic precedence.
- Compare actual layouts and allocation/capacity/peak costs. Test exact and
  one-below aggregate retained limits, including existing request, scratch and TX
  charges and later response-window admission under the selected accounting
  policy. A nominal charge is not a measured heap bound.
- Measure status-only, common small heads, many short headers and larger values,
  including the complete Gateway publication path. Keep benchmark input costs
  and allocator assumptions visible.
- Keep response windows separate. This proposal grants no response BodyStorage,
  full-response aggregation, request-body cache, new logs or metrics, dependency,
  or Gateway adapter iterator/API redesign.
- Require a separately reviewed implementation plan, leaf and Gateway WAF tests,
  and project checks before changing runtime code. Preserve canonical body and
  observability ownership and the leaf's dependency boundary.

This proposal is unimplemented and unmeasured; it awaits user decision.
