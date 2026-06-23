---
name: cross-kind-dependency-ordering-not-enforced
description: Use when reviewing findings that compare Edgion against Envoy/Istio xDS warming and claim "no cross-kind dependency ordering / routes applied before backends arrive / HTTPRoute installed before its Service/EndpointSlice / Gateway listener starts before EdgionTls cert / each kind synced by an independent Watch task with no sequencing / make-before-break missing". Covers why per-kind independent sync is an accepted tradeoff and why the transient window is bounded, self-healing, and not below the Envoy/Istio default baseline.
status: accepted-tradeoff
finding_id: sync-02
closed: 2026-06-14
---

# No cross-kind dependency ordering — routes can be applied before their backends arrive — ACCEPTED-TRADEOFF (matches Envoy/Istio default baseline)

## Conclusion

Each resource kind is synced by an independent `Watch` RPC stream + `tokio::spawn`
task (`start_watch_kinds` in `edgion-gateway/src/conf_sync/conf_client/grpc_client.rs`,
`start_watch` in `edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs`), each
firing its ConfHandler `full_set`/`partial_update` as soon as its own data arrives.
There is no cross-kind dependency graph (HTTPRoute does not wait for
Service/EndpointSlice; Gateway does not wait for EdgionTls), unlike Envoy's xDS
CDS→EDS→LDS→RDS warming sequence.

This is an accepted design tradeoff, **not** a defect and **not** a narrow review
fix. The proposed "define a resource-kind dependency graph + startup barrier doing a
coordinated full-set rebuild in dependency order + runtime read-dependency that marks
unresolved backend references as 'pending' instead of installing the route" is a
large cross-layer feature (conf-sync sequencing + a new pending-route state machine +
cross-kind consistency semantics) with medium risk. If ever prioritized it must go
through brainstorming as a `tasks/todo/` feature, not this workflow.

## Core Rationale

**1. The startup window is already gated; the real gap is only runtime incremental updates**

Pingora does not accept traffic until startup step 21 (`create_and_configure_server`
+ `run_forever`, `edgion-gateway/src/cli/mod.rs`). Step 18 `wait_for_ready` blocks
until every kind has completed its first List, and step 19 `preload_load_balancers`
warms the LB pools — both *before* traffic flows. Early route rebuilds during the
List phase observe an incomplete world, but backend resolution is a request-time
lookup by `service_key` against the (by-then-populated) cache, not baked into the
route table, so the transient state is corrected before the first request. The
finding itself concedes "by then all caches have data." The only genuine gap is the
**runtime incremental** case: an HTTPRoute update that references a brand-new Service
whose EndpointSlice Watch event has not yet been processed by its independent task.

**2. The exposure is bounded and self-healing**

A route that resolves to zero backends returns a transient 503 — `backends/mod.rs`
(`get_peer` failure path → `end_response_503`, with the in-code comment "503 for
transient backend resolution failures (no endpoints, address errors)"). When the
lagging kind's Watch task catches up (typically sub-second), the LB pool is refreshed
via ArcSwap and subsequent requests succeed. There is no permanent breakage, no
poisoned route table, and no operator action required.

**3. Not below the Envoy/Istio default baseline**

Envoy's make-before-break warming holds within a **single** xDS management server
(istiod) that controls delivery order across CDS/EDS/LDS/RDS. Edgion's structural
equivalent is the startup `wait_for_ready` barrier, which does gate process-start
traffic. For runtime updates, Envoy/Istio data planes still return 503 (no healthy
upstream) when a route references a Service whose endpoints have not yet propagated
via EDS — the eventual-consistency edge exists there too and is also bounded by
propagation latency. So Edgion is at parity with the default baseline, not behind it.
This finding is the cross-kind-consistency sibling explicitly cross-referenced by
`gateway-no-local-config-snapshot.md` (sync-05) and shares the Envoy-comparison
framing of `listener-port-set-frozen-at-startup.md` (listener-01).

## Fix Suggestions Not Accepted

- "Define a resource-kind dependency graph and hold off `full_set` on higher-level
  kinds until all dependency kinds finished their first List" — a startup barrier of
  this kind duplicates what `wait_for_ready` + request-time backend resolution
  already provide (traffic does not flow until all caches are ready), so it buys
  little for large cross-layer cost.
- "Make the route handler's `partial_update` check Service/EndpointSlice caches for
  referenced backends and mark unresolved references as 'pending' instead of
  installing the route" — a new pending-route state machine plus cross-kind read
  dependencies is a wide-impact feature touching the conf-sync core; it also risks
  routes that never un-pend if the producing event is missed. Promote to
  `tasks/todo/` via brainstorming if prioritized.
- "Treat the transient 503 window as a high-severity regression vs Envoy" — rejected
  on the facts: Envoy/Istio exhibit the same bounded, self-healing 503 on EDS lag
  during incremental updates (see Rationale 3).

## Re-evaluation Triggers

Re-open (as a `tasks/todo/` feature, not a review fix) only if:

- A production incident shows the runtime incremental-update window causing a
  measured, non-transient outage (e.g. routes stuck at zero backends well beyond
  Watch-propagation latency) that multi-replica + readiness probes cannot cover.
- The project adds an in-Gateway resource-dependency model for another reason, making
  cross-kind ordering an incremental rather than greenfield change.
- The conf-sync protocol moves to a single multiplexed stream with server-controlled
  ordering, at which point warming becomes cheap to enforce.

## Reference Cases

- `tasks/review-envoy/sync-02-no-cross-kind-ordering.md` (closed 2026-06-14, ⚠️ accepted-tradeoff)
- `edgion-gateway/src/conf_sync/conf_client/grpc_client.rs` (`start_watch_kinds` — per-kind watch, no sequencing)
- `edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs` (`start_watch` — independent `tokio::spawn` per kind)
- `edgion-gateway/src/cli/mod.rs` (`wait_for_ready` step 18 + `preload_load_balancers` step 19 gate traffic before `run_forever`)
- `edgion-gateway/src/backends/mod.rs` (transient zero-backend → `end_response_503`, self-healing on LB ArcSwap refresh)
- `skills/04-review/architecture/gateway-no-local-config-snapshot.md` (sync-05 — sibling Envoy-comparison tradeoff that cross-references this finding)
- `skills/04-review/architecture/listener-port-set-frozen-at-startup.md` (listener-01 — sibling startup/Envoy-comparison limitation)
