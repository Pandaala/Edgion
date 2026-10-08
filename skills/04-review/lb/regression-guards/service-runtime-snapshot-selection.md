---
name: service-runtime-snapshot-selection
description: Use when reviewing Service backend selection health and outlier-state lookup cost, identity stability, or panic-mode eligibility side effects.
knowledge_type: regression-guards
---

# Service selection captures one live runtime snapshot

## Conclusion and scope

`CapturedServiceSelection` captures one actual Service projection, checked section
and membership-matched `Arc<ServiceRuntimeSnapshot>` for a native HTTP/gRPC peer
attempt. `select_service_endpoint` retains the same live capture for L4/mirror/custom
callers and delegates to the shared selection body. Panic-threshold survivor counting
and the subsequent selector use that same snapshot. Candidate eligibility reads
the precompiled `EndpointRuntimeIdentity` from `LbBackend`, looks it up in the
snapshot's endpoint map, and then reads the endpoint's live health and outlier
state. It does not query the top-level health or outlier Service maps and does not
recompute a backend hash.

This rule covers Service endpoint selection in
`edgion-gateway/src/backends/selector.rs`. It does not freeze endpoint health into
an `EdgionBackendTrafficPolicy` configuration generation or prescribe the health
probe and passive-ejection algorithms.

## Rationale and evidence

The original `lb-selection-service-runtime-state-capture` task described a hot
path where each candidate predicate looked up its Service in both global stores,
then looked up the endpoint. A panic-threshold scan could repeat that work before
actual selection.

The current discovery coordinator publishes an `AvailableServiceProjection`
whose `ServiceRuntimeCell.active` holds the runtime snapshot. The selection path:

1. loads the joint discovery generation and Service projection once;
2. loads `runtime_cell.active` once and verifies its `membership_identity`;
3. passes the captured snapshot to both `degrade_engaged` and
   `runtime_eligible`;
4. resolves candidates through their immutable `EndpointRuntimeIdentity`.

Native HTTP/gRPC also derives actual Service type, ExternalName hostname, annotations,
port/protocol/appProtocol and sticky revalidation from that capture. No Service
reload follows DNS await. A separate retry captures the current projection while
request EBTP/TLS capture, including absence, remains unchanged. The ordinary healthy
capture/selection allocation test is narrowly scoped; it does not erase the existing
residual expiry/rejection costs below.

`EndpointRuntimeState.health_eligible` and `outlier_ejected` read atomically
updated state, so health transitions and ejections remain visible without
rebuilding the selector. Membership and policy identities reject late
completions; retired endpoint state is detached and frozen so an in-flight old
selection cannot widen after replacement.

Survivor counting calls `runtime_eligible(..., false)` and therefore does not
increment rejection counters. Ordinary candidate rejection calls it with `true`.
Lazy outlier expiry uses `clear_outlier_ejected`, whose atomic swaps balance the
ejected-endpoint gauge at most once. The degrade branch deliberately bypasses
health and outlier rejection after the threshold decision.

Authority: `skills/00-design-rules.md` (hot-path restraint and cache
unification) and `skills/01-architecture/02-gateway/07-backends.md` (canonical
Service discovery and runtime-identity lifecycle).

## Alternatives and limits

Restoring independent `HealthStatusStore` and `OutlierDetectionStore` lookups in
the candidate predicate would reintroduce per-candidate Service-map access and
could mix state across membership generations. Copying health or ejection bits
into an immutable policy snapshot would make updates stale. The current snapshot
instead fixes membership while retaining atomically updated endpoint state.

One selection may still inspect an endpoint more than once when panic survivor
counting precedes ordinary selection. The guarantee is one Service-level runtime
capture and no repeated identity/hash construction, not one endpoint-state read
per request. This is not a lock-free or allocation-free selection guarantee.

### Accepted residual cost

On 2026-09-08, after reviewing the remaining locking and allocation costs, the
user explicitly chose to close the task without further implementation. The
original repeated Service lookup defect is corrected; the task's stronger
no-blocking-lock and no-per-candidate-allocation criteria remain unmet and are
accepted for this existing path, not declared fixed or made a general exemption.

`EndpointRuntimeState::outlier_ejected` takes `completion_lock` at cooldown expiry
and calls `PassiveEjectionState::expire_legacy_ejection`, which takes its own
state mutex. This keeps passive recovery, the runtime gate, gauge accounting,
and retirement coordinated. A competing failure completion can hold the endpoint
lock while waiting for `ServiceCbState::eject_lock` and scanning endpoint states
for percent arbitration. Waiting time is therefore not proven short. Ordinary
healthy and unexpired eligibility reads avoid this recovery lock.

`record_cb_rejection` and `record_cb_recovery` in
`edgion-gateway/src/observe/metrics/registry.rs` allocate the Service label with
`service_key.to_string()`. Rejection can occur per candidate; recovery runs on
the transition edge. These costs remain real even though identity construction
and top-level Service lookups have been removed.

The accepted benefit is preserving the existing recovery/retirement protocol
without a broad state-machine redesign for an unmeasured performance gain.
Removing the lock alone risks stale recovery clearing a newer ejection;
`try_lock` with continued rejection can delay recovery and change panic decisions,
while early admission with deferred cleanup separates availability from passive
state and gauge accounting. Background recovery adds scheduling and lifecycle
costs. Cached metric handles need a separate lifetime and exporter-idle review.
No contention benchmark or production latency measurement supports a stronger
claim about the remaining cost.

Policy capture coherence is separate: HTTP/gRPC now pass the captured override
while L4/mirrors use the runtime threshold. See the
[captured Service policy rule](../../backends/regression-guards/ebtp-atomic-service-snapshot.md).
The residual-cost acceptance above does not waive that requirement.

## Verification and re-evaluation

Historical verification recorded on 2026-09-07 (not rerun for the 2026-09-08
documentation-only decision):

- `cargo test -p edgion-gateway --lib backends::discovery::coordinator::tests::`:
  24 passed. This covers late health and outlier completions, membership
  replacement, policy removal, detached-state freezing, and completion versus
  retirement ordering.
- `cargo test -p edgion-gateway --lib backends::selector::tests::`: 9 passed,
  including panic-selection metric ownership and lifecycle cases.

The 2026-09-08 review traced selection, endpoint completion, passive expiry,
retirement, and rejection/recovery metrics in current source. No production
behavior or user-facing configuration changed. Repository-wide closing checks
are reported by the task runner separately; the historical unit-test results
above are not fresh verification.

Re-evaluate if candidate eligibility stops receiving the captured
`ServiceRuntimeSnapshot`, a selector performs a live top-level health/outlier
store lookup, endpoint identity becomes request-built again, or health/outlier
state stops using identity-guarded atomic runtime objects. Revisit the accepted
residual cost if profiles show selection latency at cooldown expiry, synchronized
recoveries or large-Service arbitration cause contention, rejection-label
allocation becomes material, or recovery/retirement semantics are redesigned.

## Resolution references

- `lb-selection-service-runtime-state-capture` — previously deferred performance
  defect: repeated lookup and identity construction were already corrected by
  the Service discovery coordinator/runtime identity redesign. On 2026-09-08
  the user accepted the remaining locking/allocation costs and requested closure
  without further code changes. This supersedes the earlier blanket closure
  description; it does not claim every original acceptance criterion passed.
