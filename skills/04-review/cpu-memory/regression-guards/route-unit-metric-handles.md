---
knowledge_type: regression-guards
---

# `record_backend_request` / `record_request_duration` per-request allocations

## Status: Resolved by route-unit metric handle slots.

## Finding (canonical form)

`record_backend_request` and `record_request_duration` used to build a fresh
`Vec<(&'static str, String)>` per request and let `metrics-rs` resolve the
counter / histogram handle from scratch (`Vec` alloc + `to_string()` × ≤ 18 +
`IntoLabels::collect` + `SharedString::Owned` × N). The hot path also paid one
hashmap lookup per metric to find the already-registered handle.

## Decision

Adopted. Pre-resolved `DynamicCounter` / `DynamicHistogram` descriptor matrices are attached to
`HttpRouteRuleUnit` / `GrpcRouteRuleUnit` and indexed by
`(gateway_ns, gateway_name) -> (backend_ref_idx, protocol_idx, status_idx)`.
See `docs/superpowers/specs/2026-06-10-observe-f002-route-unit-metric-handles-design.md`
for the design and `docs/superpowers/plans/2026-06-10-observe-f002-route-unit-metric-handles.md`
for the landed plan.

Integration-testing-mode, route-miss, slotless bypass paths such as
ExternalJump, and unknown protocols fall back to a `Vec<Label>` + `Label::new`
rewritten version of the original functions. DirectEndpoint retains the raw
backendRef slot and can use the handle path. Warmed, non-renewing operations use 0 allocations,
no identity construction or registry lookup after first use, with short per-slot
operation gates coordinating updates with idle expiration. Fixed writes remain
raw exporter atomics. No whole-system QPS conclusion follows from component
benchmarks.

`attached_gateways` is the list of `(gateway_ns, gateway_name)` tuples
resolved from `parent_refs` at conf-load time (via
`gateways_from_parent_refs` in `edgion-gateway/src/routes/common.rs`).
HTTP Service/invalid selection carries `CompiledBackend.slot`; AI selection
carries `AiPeerTarget.backend_slot`; DirectEndpoint retains the validated raw
slot; and the gRPC initialization path derives its slot from the rule refs.
HTTP route matrices are aligned to every raw backendRef slot, including
`EdgionBackend` and invalid selected refs. Backend metrics label only the
resolved namespace/name, not resource kind, so each corresponding handle cell
has the same label identity as the dynamic fallback. Preparation creates private
renewable descriptors for configured gateway/route/backendRef combinations, not
zero-valued export cells. Only the first event admits a lifetime key and creates
accumulation; renewal can allocate bounded aggregation after idle expiration.
Warmed AI writes avoid label allocation and registry lookup.

## Why future review should not re-report this

Warmed, non-renewing writes are structurally zero-allocation. There is no allocation left to
remove from the metric write. Further improvement would require either
(a) collapsing the gateway / backend lookups into an `O(1)` array dereference
(would require Gateway-API model changes beyond a metric concern), or
(b) eliminating the small `&str` equality checks entirely (negligible win
relative to the rest of the request path).

If a profiler ever shows the linear `attached_gateways` scan dominating CPU
under unusually high parent_refs cardinality, file a new finding referencing
this case file with the measured numbers.

## Resolution references

- `gw-15` — 2026-08-09.

## Cached lifetime boundary (ARCH-043)

Matrices retain private renewable proxies, never exporter accumulation handles.
Preparation can bind the recorder context but does not admit a key or create an
export epoch. First actual recording admits a stable lifetime slot. Scraping can
clear idle accumulation while the same route matrix remains alive; the next
record renews that same admitted slot with reset counter/histogram statistics.
Dynamic expiration does not refund admission. Direct matrix tests exercise this
production path, while integration testing uses the logging fallback. Treat the
historical zero-lock claim as superseded; request allocation and gate latency
must be verified against the current backend.
