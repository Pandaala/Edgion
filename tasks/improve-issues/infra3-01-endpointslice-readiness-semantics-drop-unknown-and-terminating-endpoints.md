# infra3-01: EndpointSlice readiness treats unknown `ready` as not-ready and never falls back to serving-terminating endpoints

- **Category**: design-error
- **Severity**: P2
- **Scope**: local
- **Location**: `edgion-gateway/src/backends/discovery/coordinator.rs:1625-1631,1655,1878-1900`,
  `edgion-gateway/src/backends/discovery/endpoint_slice/discovery_impl.rs:24`,
  `edgion-gateway/src/backends/selector.rs:296-302,328`,
  `edgion-gateway/src/backends/discovery/endpoint_slice/ep_slice_store.rs` (`ready_endpoints_for_port`)

## Summary
The Gateway derives endpoint eligibility only from `EndpointConditions.ready`, and it maps an absent
(`nil`) value to `false`. The Kubernetes API contract says the opposite: a nil `ready` means "unknown" and
consumers should treat it as ready. The Gateway also ignores the `serving` and `terminating` conditions, so it
has no fallback when every endpoint of a Service is terminating. Kubernetes (KEP-1669) and Envoy-based gateways
do have that fallback. Two outages follow. Manually managed or third-party EndpointSlices that omit
`conditions` are never routable. A Service whose pods are all terminating, but still serving, returns 503 for
the rest of their graceful-drain window.

## Current behavior (evidence)
- `coordinator.rs:1625-1631`: `let ready = endpoint.conditions.as_ref().and_then(|c| c.ready).unwrap_or(false);`.
  `serving` and `terminating` are never read anywhere in `edgion-gateway/src/backends/` (a grep for
  `serving`/`terminating` finds only `serving_port`).
- `coordinator.rs:1655`: a socket is `ready` if any origin is ready. `coordinator.rs:1878-1885` derives
  `ready_slots` / `ready_backends` with `.filter(|slot| slot.ready)`. `coordinator.rs:1900` builds every LB
  selector (RR, CH, LeastConn, EWMA) from `ready_backends` only.
- The degrade/panic path (`selector.rs:296-300`) only relaxes the *health* filter (`|_| true`) over the same
  ready-only selector. It cannot reach a non-ready slot. Sticky targets require `slot.ready`
  (`selector.rs:328`).
- `discovery_impl.rs:24` (`EndpointSliceExt::build_backends`) repeats the same `unwrap_or(false)` rule.
  Outbound plugin targets (`plugins/http/common/outbound/target.rs:61`, `pinned_resolver.rs:76`) and the
  health manager consume `ready_endpoints_for_port`, which reads `ready_slots`.
- Result: an EndpointSlice with `conditions: {}` or no `conditions`, which is common for hand-written slices
  that point at external IPs (label `endpointslice.kubernetes.io/managed-by` set to a custom value), yields a
  Service with zero eligible endpoints. Requests fail with the empty-endpoint 503 path. A Service in a
  `Recreate` rollout, a single-replica Deployment being replaced, or a pod draining under `preStop` has
  `ready=false, serving=true, terminating=true` endpoints only. Edgion drops them immediately even though
  the pods still accept traffic.

## Industry comparison
- Kubernetes API (`discovery.k8s.io/v1` `EndpointConditions.ready`): "A nil value should be interpreted as
  'true'" (the unknown state is treated as ready).
- KEP-1669 "ProxyTerminatingEndpoints" (GA in Kubernetes 1.28). When a Service has no ready endpoints,
  kube-proxy routes to endpoints that are `serving && terminating`, so connections drain gracefully during
  rollouts.
- Istio/Envoy (EDS), Envoy Gateway and Cilium follow the same rule. They use `serving` for eligibility and
  fall back to terminating-but-serving endpoints only when no ready endpoint remains.

## Impact
- Correctness: valid EndpointSlices (nil `ready`) never receive traffic. Nothing reports this, because the
  Service and slice are both "Available".
- Availability: 503s for the whole drain window of all-terminating Services. That window is exactly when
  Kubernetes expects proxies to keep using the draining endpoints. Edgion behaves worse than kube-proxy, so
  traffic sent through the Gateway fails where ClusterIP traffic would succeed.

## Suggested direction
1. Normalize the three conditions once in `normalize_sections`: `ready = conditions.ready.unwrap_or(true)`,
   `serving = conditions.serving.unwrap_or(ready)`, and `terminating = conditions.terminating.unwrap_or(false)`.
   Carry `serving`/`terminating` on `NormalizedBackendSlot` and include them in `slot_proof`.
2. Selection: build selectors from ready slots as today. When a section has zero ready slots, build the
   selector set from `serving && terminating` slots instead, following KEP-1669. Keep this fallback separate
   from the health-based degrade threshold.
3. Apply the same normalization in `EndpointSliceExt::build_backends`, or delete that helper if it has no
   production caller.
4. Add the slot origin (ready vs. terminating-fallback) to the access log's upstream block so operators can
   see when fallback traffic is in use.

## Notes
Checked `skills/04-review/backends/` (SKILL index, `regression-guards/discovery-normalized-geometry-ownership.md`,
`ep-slice-no-port-fail-closed.md`, `named-port-resolve-against-originating-slice.md`). None of them define
readiness semantics or decide to treat nil `ready` as false. No `tasks/` item covers `serving`/`terminating`.
Related but distinct: `gw-infra-02` (state reset on `ready` flips) and `gw-core-01` (Gateway's own shutdown
readiness). Pre-release: no compatibility concern.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P2 unchanged.
- `backends/discovery/coordinator.rs:1625-1630`: `conditions.ready.unwrap_or(false)`. `endpoint_slice/discovery_impl.rs:24` repeats it.
- A grep of `backends/` for `.serving` or `.terminating` finds no read; the only hit is `serving_port`. No KEP-1669 fallback exists.
- The Kubernetes `EndpointConditions.ready` contract does say nil should be interpreted as true. The controller does not normalize third-party slices; only its own DNS slice builder sets conditions (`edgion-controller/src/services/dns_resolver/slice_builder.rs:167`).
- No `skills/04-review/backends` or `lb` page decides readiness semantics.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P2 unchanged.
- `backends/discovery/coordinator.rs`, `endpoint_slice/discovery_impl.rs` and `backends/selector.rs` are unchanged between feature-08-30 and HEAD.
- `coordinator.rs:1626-1630` and `discovery_impl.rs:24` still map nil `ready` to `false`; a grep of `backends/` for `.serving` / `.terminating` still finds no read, so no KEP-1669 fallback exists.
