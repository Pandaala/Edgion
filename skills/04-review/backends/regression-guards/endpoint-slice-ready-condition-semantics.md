---
name: endpoint-slice-ready-condition-semantics
description: Use when reviewing how Gateway backend discovery interprets EndpointSlice endpoint conditions (ready / serving / terminating), nil ready handling, or hand-written and sync-bridge EndpointSlices that omit conditions.
knowledge_type: regression-guards
---

# EndpointSlice ready condition semantics

## Conclusion and scope

Gateway Service discovery (`normalize_section` in
`edgion-gateway/src/backends/discovery/coordinator.rs`) derives endpoint readiness
from `endpoints[].conditions.ready` once per endpoint:

- `ready: true` -> ready.
- `ready` nil (no `conditions`, empty `conditions`, or `ready` key absent) -> ready.
- `ready: false` -> not ready.

The ready bit feeds `EndpointOrigin.ready`, slot readiness (any origin ready),
`slot_proof`, `ready_slots`, and the four eager selectors built by
`materialize_section`. Unready slots remain in `all_slots` for active health
probes, sticky lookup and DirectEndpoint authorization, as before.

The `serving` and `terminating` conditions are currently not read. The KEP-1669
serving-terminating fallback is a deferred feature, tracked in
[`tasks/todo/endpointslice-terminating-endpoint-fallback.md`](../../../../tasks/todo/endpointslice-terminating-endpoint-fallback.md).
Its absence is a known gap, not an accepted design.

## Rationale and evidence

Authority: Kubernetes `discovery.k8s.io/v1` `EndpointConditions.ready`
("A nil value should be interpreted as true"). kube-proxy applies the same rule
(`Ready == nil || *Ready`), so a hand-written slice behaves the same for
ClusterIP traffic and Gateway traffic.

Hand-written and sync-bridge slices are a supported input: the
[service discovery guide](../../../../docs/en/ops-guide/infrastructure/service-discovery.md)
has Consul, Nacos, Eureka and ZooKeeper bridges write EndpointSlices directly, and
those writers commonly omit `conditions`. The Kubernetes EndpointSlice controller
always sets `ready` explicitly, so pod-backed slices are unaffected. The Controller
does not strip or rewrite conditions; the only Edgion-produced slices (DNS slices,
`edgion-common/src/dns/endpoint_slices.rs`) set all three conditions explicitly.

## Alternatives and limits

Root cause: `normalize_section` used `.unwrap_or(false)`, so a valid slice that
omitted `ready` produced a Service with zero eligible endpoints and an
empty-endpoint 503, with no status or log signal. A caller-less duplicate
(`EndpointSliceExt::build_backends` in `endpoint_slice/discovery_impl.rs`) repeated
the rule; it was deleted rather than corrected.

Rejected alternative: keep strict mode (require explicit `ready: true`) and warn.
It contradicts the API contract and kube-proxy, and EndpointSlice has no status
surface, so a warning would still leave the Service silently unroutable.

Limit: a nil-ready endpoint from a sync bridge is a write-time snapshot. Liveness
between syncs comes from `EdgionBackendTrafficPolicy` active health checks and
outlier detection, which apply to these slots like any other. A `ready: nil,
terminating: true` endpoint is ready under the current contract.

## Verification and re-evaluation

Regression test: `nil_ready_condition_is_interpreted_as_ready_and_selectable` in
`edgion-gateway/tests/unit/backends/discovery/coordinator_test.rs` covers missing
conditions, empty conditions, nil ready with other conditions set, explicit ready
and explicit unready, and asserts slot/origin readiness, `ready_slots` and all four
selector backend counts. On 2026-11-12 it failed against the old `unwrap_or(false)`
(`missing conditions`: left false, right true) and passed with the fix; the
`backends::discovery` unit tests passed (122 cases), and the targeted gateway
backends/lb/outbound/DirectEndpoint/AllEndpointStatus/response-cache/session/UDP
unit tests passed (875 cases).

Integration: `HTTPRoute/Basic` case `nil_ready_endpoint_routable` routes
`nil-ready.example.com` to `EndpointSlice_nil-ready-backend.yaml`, the only
integration fixture slice that omits `ready`. It exercises nil `ready` only in
FileSystem mode: K8s mode (`generate_k8s_conf.sh`) skips EndpointSlice fixtures and
lets Kubernetes generate explicit `ready: true` slices, and a direct (no Gateway)
run passes trivially. Executed on 2026-11-12 in FileSystem mode on macOS:
`run_integration.sh -r HTTPRoute -i Basic` passed 7/7 including this case, with the
dead-loop, unknown-field and cache-residue checks passing. The case was not run
against the pre-fix code; the unit test above carries the red/green evidence.

Re-evaluate when the KEP-1669 fallback lands (normalization of `serving` /
`terminating`, fallback selectors, sticky eligibility), when readiness
normalization moves out of `normalize_section`, or when the Controller starts
rewriting EndpointSlice conditions.

## Resolution references

- infra3-01 (`tasks/improve-issues/`, P2, 2026-11-12): nil `ready` now interpreted
  as ready; dead `EndpointSliceExt` removed; terminating fallback deferred to the
  task above. User docs gained an "Endpoint Readiness" section in
  `docs/{en,zh-CN}/ops-guide/infrastructure/service-discovery.md`.
