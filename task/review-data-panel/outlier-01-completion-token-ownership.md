# Evaluate Service outlier completion-token ownership

Status: deferred for user decision. No runtime change.
Inspected baseline: `b94f2e1d2`.

## Evidence and decision

`backends/outlier_detection/store.rs` captures an OutlierCompletionToken containing
only an Arc to OutlierPolicyIdentity. HTTP backend selection and the TCP,
terminated TLS and passthrough TLS paths retain that token. Success and failure
completion look up the Service state again, clone its Arc and compare the policy
identity. The coordinator then validates endpoint membership and policy under
the endpoint completion lock. These Arc clones increment reference counts;
they do not clone the state payload or allocate a new state.

The success path also clones the endpoint state before entering coordinator
validation. Keeping its DashMap read guard instead is unsafe: it would acquire
endpoint-map access before completion_lock, whereas coordinator activation holds
completion_lock while remove_runtime_identity removes the endpoint-map entry.
The existing clone releases the map guard before that lock acquisition. Failure
lookup similarly releases its guard before insertion fallback, state accounting
and percent arbitration. Do not implement a mechanical clone-to-borrow rewrite.

One possible redesign would capture the Service state owner in the completion
token, removing the later Service lookup and ownership acquisition. This would
also retain its config and identity-state map through outstanding tokens after
policy replacement/removal. Multiple tokens would share a generation, not copy
its map per request, but delayed requests can retain several retired generations.
The present token retains only policy identity. Any candidate must account for
that changed lifetime and the policy publication windows between coordinator
updates and Service-map insertion/removal.

Choose whether to retain current ownership or investigate a generation-bound
completion handle. No representation, lookup saving or net memory/CPU benefit
has been selected or measured. Endpoint-state ownership and successful ejection
publication are separate from eliminating the Service lookup.

## Acceptance for a selected investigation

- Trace capture and completion across HTTP, TCP and both TLS paths. Preserve
  missing-policy behavior, policy pointer/generation checks, membership checks,
  stale completion rejection and the order of accounting versus retry decisions.
- Preserve registration only after coordinator validation, no resurrection after
  retirement, recovery/ejection ordering and once-only gauge settlement.
  Preserve threshold candidate revalidation, current-membership percent bounds
  and service arbitration. Do not replace completion locks with snapshot checks.
- Review publication windows, config disable/re-enable, endpoint removal and
  recreation, same-address identity replacement and long-lived old requests.
  A retained handle must not make an old policy executable after revocation.
- Keep DashMap guards out of conflicting completion/retirement lock order.
  Explicitly review any changed guard lifetime; removing an Arc is not proof
  that a borrowed replacement is safe or faster.
- Measure token capture and success/failure completion separately, including
  absent state, many endpoints, reload frequency and delayed completions.
  Compare lookup/refcount work, token size and retired-generation retained bytes;
  do not equate shared ownership with a copied map or lower allocation counts.
- Reuse canonical coordinator and passive-ejection ownership. Add no global
  cache, new metric, request tracing or independent state machine. Freeze an
  exact implementation allow-list and obtain independent IMPL plan/final review.

Relevant original knowledge:
[current-membership percent caps](../../skills/04-review/backends/regression-guards/circuit-breaker-record-failure-zero-total-endpoints.md),
[threshold-only arbitration](../../skills/04-review/cpu-memory/regression-guards/outlier-ejection-arbitrates-only-threshold-candidates.md)
and [snapshot/completion ownership](../../skills/04-review/lb/regression-guards/service-runtime-snapshot-selection.md).
Sources are `backends/outlier_detection/store.rs` and
`backends/discovery/coordinator.rs` under `edgion-gateway/src/`.

Fresh baseline verification passed 36 outlier, 41 coordinator, nine connect-failure
and 11 proxy-error tests (97 total). These verify existing behavior, not a
proposed ownership redesign or its contention and retention costs.
