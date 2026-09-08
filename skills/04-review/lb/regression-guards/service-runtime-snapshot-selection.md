---
name: service-runtime-snapshot-selection
description: Use when reviewing Service backend selection health and outlier-state lookup cost, identity stability, or panic-mode eligibility side effects.
knowledge_type: regression-guards
---

# Service selection captures one live runtime snapshot

## Conclusion and scope

`select_service_endpoint` captures one `Arc<ServiceRuntimeSnapshot>` for the
selected Service before evaluating candidates. Panic-threshold survivor counting
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
per request. Expired outlier cooldown is a cold edge that may briefly take the
endpoint completion lock to settle passive state and metrics.

## Verification and re-evaluation

Fresh verification on 2026-09-07:

- `cargo test -p edgion-gateway --lib backends::discovery::coordinator::tests::`:
  24 passed. This covers late health and outlier completions, membership
  replacement, policy removal, detached-state freezing, and completion versus
  retirement ordering.
- `cargo test -p edgion-gateway --lib backends::selector::tests::`: 9 passed,
  including panic-selection metric ownership and lifecycle cases.

No production behavior changed while closing the stale task. The repository-wide
closing checks are reported by the task runner rather than duplicated here.

Re-evaluate if candidate eligibility stops receiving the captured
`ServiceRuntimeSnapshot`, a selector performs a live top-level health/outlier
store lookup, endpoint identity becomes request-built again, or health/outlier
state stops using identity-guarded atomic runtime objects.

## Resolution references

- `lb-selection-service-runtime-state-capture` — previously deferred performance
  defect, found already corrected by the Service discovery coordinator/runtime
  identity redesign on 2026-09-07.
