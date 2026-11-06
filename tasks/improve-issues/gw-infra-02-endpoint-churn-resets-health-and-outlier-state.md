# gw-infra-02: Any EndpointSlice change resets health/outlier state for the whole Service and opens a 503 window

- **Category**: design-error
- **Severity**: P1 (significant)
- **Scope**: large-direction
- **Location**: `edgion-gateway/src/backends/discovery/coordinator.rs:958-1060` (`transact`), `:1070-1149` (`activate_service`), `:369-397` (`EndpointRuntimeState::fresh`), `:1716-1760` (`slot_proof`), `:1802-1815` (`projection_membership_identity`), `:1837-1902` (`materialize_section`); `backends/selector.rs:46-53,194-209` (`load_active_projection`, `CapturedServiceSelection::capture`); `backends/health/check/manager.rs:240-275` (completion recording); `lb/runtime_state/mod.rs:205-212` (`begin_draining`)

## Summary
Per-endpoint runtime state (active-HC eligibility and consecutive counters, passive outlier ejection and
backoff history) is keyed by an identity that is regenerated for *every* endpoint of a Service whenever *any*
slot in that Service changes (one pod added/removed, one `ready` flip). All endpoints restart as healthy and
un-ejected, in-flight probe results are discarded, and the Service is briefly published with an inactive
runtime cell so requests fail with "endpoint not found". Under rolling deploys or autoscaling, active health
checking and outlier detection are effectively disabled for exactly the Services that are changing.

## Current behavior (evidence)
- `slot_proof` hashes the Service UID/epoch, port identity, socket, `ready` and origins. 
  `projection_membership_identity` hashes all proofs of the Service, so a single slot change produces a new
  membership identity.
- `materialize_section` assigns a fresh `incarnation` epoch to *every* slot and builds a new `LbBackend` with a
  new `EndpointRuntimeIdentity`; only `runtime_counters` (conn/EWMA) are carried over from the old backend by
  proof or socket.
- `activate_service` (`:1076`) first stores `ServiceRuntimeSnapshot::inactive()`, then builds new
  `EndpointRuntimeState::fresh(...)` for every endpoint (`health_eligible = true`, `outlier_ejected = false`,
  consecutive counters 0), freezes all old entries, calls `remove_runtime_identity` on the outlier store for
  every old identity (dropping `num_ejections` backoff), and for each old backend with live connections calls
  `begin_draining` (a `format!` key, a DashMap insert and an `info!` log per endpoint).
- `record_health_completion` / `with_current_endpoint_locked` reject any probe result whose
  `membership_identity` differs from the current one, so a probe cycle that straddles a membership change
  is lost; the next cycle starts from zero counters. A dead pod in a Service whose membership changes more
  often than `unhealthy_threshold × interval` is never marked unhealthy.
- `transact` publishes the new generation (`self.published.store(next)`) *before* activating changed
  Services, and `activate_service` stores an inactive snapshot before building the new one. During that window
  `CapturedServiceSelection::capture` sees `active.membership_identity != Some(available.membership_identity)`
  and returns `BackendEndpointSliceNotFoundByRoundRobinDefault` (fail-closed). The window grows with the number
  of endpoints (per-endpoint lock + outlier-store removal + draining insert + log) and with the number of
  changed Services in one batch (they are activated sequentially; the last one waits for all earlier ones).
- Because every surviving endpoint gets a new incarnation, `DRAINING_BACKENDS` receives an entry for every
  still-live endpoint that had in-flight connections at each change. With long-lived HTTP/2, gRPC or WebSocket
  connections, a rolling deploy of an N-pod Service accumulates O(N × changes) "draining" entries for
  endpoints that never left, and `draining_stats` misreports them.

## Industry comparison
- Envoy `BaseDynamicClusterImpl::updateDynamicHostList` keeps existing `Host` objects for unchanged addresses
  across EDS updates, so health flags, outlier ejection state and ejection multipliers survive membership
  churn. Only added hosts are new; when active HC is configured they start as
  `FAILED_ACTIVE_HC | PENDING_ACTIVE_HC` and are excluded until the first successful check. The new priority
  set is swapped in atomically; there is no "inactive" interval.
- Kong's healthchecker keys target health by `ip:port[:hostname]` and preserves it across balancer rebuilds.

## Impact
- During rolling updates, scale events and readiness flapping, which are the periods when backends are most
  likely to be unhealthy, Edgion sends traffic to endpoints it had already marked unhealthy or ejected. It
  then has to rediscover the failure, which takes at least `unhealthy_threshold` probe intervals or
  `consecutive_*` failed requests.
- Brief fail-closed errors on healthy Services on every endpoint change, visible as sporadic 5xx during deploys.
- New endpoints are trusted immediately (fail-open) even when active HC is configured.
- Unnecessary per-change work and log volume proportional to Service size.

## Suggested direction
Recommended: separate *slot identity* from *membership generation*.
1. Key `EndpointRuntimeState` by a stable per-Service endpoint identity, either the socket plus port identity
   or the slot proof without the `ready` bit and origin list. On activation, carry over the existing
   `EndpointRuntimeState` (health eligibility, completion counters, outlier deadline and `PassiveEjectionState`)
   for every endpoint whose stable identity is unchanged. Create fresh state only for genuinely new endpoints,
   and retire/drain only genuinely removed ones.
2. Make completion fencing per endpoint (stable identity + policy generation) instead of per Service
   membership, so probes for unchanged endpoints remain valid across membership changes.
3. Build the complete new `ServiceRuntimeSnapshot` *before* publishing, then make generation and runtime
   publication a single atomic step. One option is to place the runtime snapshot inside the published
   projection. Another is to activate first and publish second, letting selectors accept either membership.
   Remove the inactive window.
4. Optional, Envoy parity: when an active HC policy exists, start new endpoints as "pending first check", with
   a fallback to eligible when every endpoint is pending (the existing degrade threshold covers panic mode).

## Notes
Checked `skills/04-review/backends/regression-guards/endpoint-slice-ownership-transitions.md`. It requires that
*retired* endpoints are detached and that late completions for the *old* identity are rejected. It does not
require resetting *unchanged* endpoints, and the proposal keeps that guarantee for removed endpoints. Also
checked `active-health-task-status-ownership.md`, `active-hc-probes-parallel-within-service.md`,
`circuit-breaker-integration-ejection-state-self-cleaning.md` and `lb/regression-guards/service-runtime-snapshot-selection.md`.
None accepts the reset or the inactive window as a tradeoff. The only justification is the in-code comment at
`coordinator.rs:1047-1049` ("fail-closed 503, never a stale membership"). The new evidence is that the
fail-closed window and the state reset are user-visible on every endpoint change, while preserving
unchanged-endpoint state is not stale membership. Not tracked in `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P1 unchanged.
- `coordinator.rs:1070-1125` `activate_service` stores `ServiceRuntimeSnapshot::inactive()`, then builds `EndpointRuntimeState::fresh(..)` for every endpoint. Fresh state means `health_eligible=true`, `outlier_ejected=false` and zeroed counters (`:369-397`). Every old entry is frozen, `remove_runtime_identity` is called on the outlier store, and the backend is drained or removed.
- `materialize_section` (`:1837-1870`) assigns a new `incarnation` to every slot and creates a new `EndpointRuntimeIdentity`. Only `runtime_counters` are carried over (by proof or socket).
- `transact` (`:1058-1067`) publishes the generation before activation, with an explicit comment that selectors fail closed in the gap. `selector.rs:50-56` `load_active_projection` returns `None` on a membership mismatch.
- `backends/regression-guards/endpoint-slice-ownership-transitions.md:35-37` describes this activation sequence but gives no rationale for resetting unchanged endpoints. No page accepts that reset as a tradeoff.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P1 unchanged.
- `backends/discovery/coordinator.rs`, `backends/selector.rs`, `backends/health/check/manager.rs` and `lb/runtime_state/` are unchanged between feature-08-30 and HEAD (the backends/lb diff is six one-line edits elsewhere).
- Cited symbols sit at the same lines: `EndpointRuntimeState::fresh` (`coordinator.rs:370`), `transact` (`:958`), `activate_service` (`:1070`), `slot_proof` (`:1716`), `projection_membership_identity` (`:1802`), `materialize_section` (`:1837`).
- The only changes to `skills/04-review/backends/` are Jiff timestamp notes on two policy pages; no page now accepts the reset or the inactive window.
