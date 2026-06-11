# `record_backend_request` / `record_request_duration` per-request allocations

## Status: Resolved by route-unit metric handle slots.

## Finding (canonical form)

`record_backend_request` and `record_request_duration` used to build a fresh
`Vec<(&'static str, String)>` per request and let `metrics-rs` resolve the
counter / histogram handle from scratch (`Vec` alloc + `to_string()` × ≤ 18 +
`IntoLabels::collect` + `SharedString::Owned` × N). The hot path also paid one
hashmap lookup per metric to find the already-registered handle.

## Decision

Adopted. Pre-resolved `Counter` / `Histogram` matrices are attached to
`HttpRouteRuleUnit` / `GrpcRouteRuleUnit` and indexed by
`(gateway_ns, gateway_name) -> (backend_ref_idx, protocol_idx, status_idx)`.
See `docs/superpowers/specs/2026-06-10-observe-f002-route-unit-metric-handles-design.md`
for the design and `docs/superpowers/plans/2026-06-10-observe-f002-route-unit-metric-handles.md`
for the landed plan.

Integration-testing-mode, route-miss, bypass paths and unknown protocols
fall back to a `Vec<Label>` + `Label::new` rewritten version of the original
functions. The hot path is 0 allocations, 0 hashing, 0 locks (linear scan
over `attached_gateways` — typically 1-3 entries — plus two atomic ops).

`attached_gateways` is the list of `(gateway_ns, gateway_name)` tuples
resolved from `parent_refs` at conf-load time (via
`gateways_from_parent_refs` in `src/core/gateway/routes/common.rs`).
`backend_ref_idx` is set by the LB selection path in
`init_backend_context_if_needed` via `backend_refs.iter().position(...)`.

## Why future review should not re-report this

The hot path is structurally zero-allocation. There is no allocation left to
remove from the metric write. Further improvement would require either
(a) collapsing the gateway / backend lookups into an `O(1)` array dereference
(would require Gateway-API model changes beyond a metric concern), or
(b) eliminating the small `&str` equality checks entirely (negligible win
relative to the rest of the request path).

If a profiler ever shows the linear `attached_gateways` scan dominating CPU
under unusually high parent_refs cardinality, file a new finding referencing
this case file with the measured numbers.
