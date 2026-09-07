---
name: cross-kind-push-no-snapshot-atomicity-controller-side
description: Use when reviewing controller/push-side findings that compare Edgion's config-sync against Envoy xDS SnapshotCache / Istio PushContext and claim "no cross-resource-kind snapshot atomicity / each ServerCache<T> has its own independent sync_version / apply_change notify_waiters fires per kind with no batch barrier / logically coupled changes arrive as separate incremental pushes / introduce a cross-kind push epoch + coalescing window / move to xDS SotW". This is the controller/push-side mirror of sync-02; the per-kind independent push is an accepted tradeoff and the push-epoch direction is a tasks/todo feature, not a narrow review fix.
status: accepted-tradeoff
finding_id: controller-04
closed: 2026-06-14
knowledge_type: intentional-design
---

# No cross-resource-kind snapshot atomicity for the data-plane push (controller/push side) — ACCEPTED-TRADEOFF

## Conclusion

Edgion's `ConfigSyncServer` maintains one `ServerCache<T>` per resource kind, each
with its own process-wide-monotonic `sync_version`
(`resource_version::next_resource_version()` —
`edgion-controller/src/conf_sync/cache_server/cache.rs:371`). Every resource-change
branch of `apply_change()` (Add/Update/Delete) emits via `push_event`, which
immediately calls `notify.notify_waiters()` (`cache.rs:275`), so logically
coupled changes that span kinds (e.g. adding an HTTPRoute that references a new
Service, or rotating a cert that touches both the Secret and the Gateway) are
delivered to Gateway watchers as separate incremental pushes with consecutive but
independent `sync_version` values. There is no "this batch of changes is complete"
barrier in the gRPC stream, unlike Envoy xDS `SnapshotCache` / Istio `PushContext`
which version all resource types into one atomic snapshot.

This is the **controller/push-side mirror** of the gateway/watch-side finding sync-02
([[cross-kind-dependency-ordering-not-enforced]]). It is an accepted design tradeoff,
**not** a defect and **not** a narrow review fix. Fix suggestions of the form
"introduce a cross-kind push epoch — drain a brief coalescing window, assign a shared
epoch tag, let the Gateway detect a complete batch" or "move to xDS SotW so the whole
state is resent atomically" are **not accepted** as review fixes.

## Core Rationale

**1. The transient window is bounded and self-healing, on a cold control path.**

The exposure is the same one analysed for sync-02: a Gateway may briefly observe a
route pointing to a Service whose endpoints have not yet been pushed, returning a
transient 503 (`backends/mod.rs` zero-backend → `end_response_503`), or a TLS
listener using the prior cert for a sub-second window. Backend resolution is a
request-time lookup by `service_key` against the cache (not baked into the route
table), so when the lagging kind's Watch task catches up the LB pool refreshes via
ArcSwap and subsequent requests succeed. No permanent breakage, no operator action.
The push path is config-sync, not the request hot path.

**2. Not below the Envoy/Istio default baseline.**

Envoy/Istio's atomic snapshot holds within a single management server controlling
delivery order; Edgion's structural equivalent is the startup `wait_for_ready`
barrier (gateway step 18), which gates process-start traffic until every kind has
completed its first List. For *runtime* incremental updates Envoy/Istio data planes
still return 503 when a route references a Service whose EDS endpoints have not yet
propagated — the eventual-consistency edge exists there too and is bounded by
propagation latency. Edgion is at parity, not behind. This shares the
Envoy-comparison framing of [[gateway-no-local-config-snapshot]] (sync-05) and
[[listener-port-set-frozen-at-startup]] (listener-01).

**3. The push-epoch fix is a large cross-layer feature, not a review fix.**

A real cross-kind epoch requires: a coalescing/drain window on the push side
(`config_sync_server.rs` / `cache.rs`), an epoch tag threaded through the per-kind
gRPC Watch streams, and a Gateway-side batch-completion detector plus a
hold-until-complete apply state machine. That is the same medium-risk cross-layer
work (conf-sync sequencing + a new pending/epoch state machine + cross-kind
consistency semantics) rejected for sync-02. If ever prioritized it must go through
brainstorming as a `tasks/todo/` feature.

## Fix Suggestions Not Accepted

- "Introduce a cross-kind push epoch: drain a ~5ms coalescing window across all
  kinds before `notify_waiters()`, assign a shared epoch tag" — large cross-layer
  feature touching the conf-sync core on both push and watch sides; promote to
  `tasks/todo/` via brainstorming, not a narrow fix.
- "Move to xDS SotW (resend the whole state atomically per push)" — eliminates
  partial-push at the cost of much higher per-push volume and a protocol rewrite;
  out of scope for a review fix.
- "Make every `apply_change` defer `notify_waiters` until all kinds for the same
  logical change are applied" — there is no controller-side notion of "the same
  logical change" across kinds (each kind has an independent workqueue worker);
  inferring batch boundaries is exactly the epoch state machine above.
- "Treat the transient window as a high-severity regression vs Envoy" — rejected on
  the facts: Envoy/Istio exhibit the same bounded, self-healing 503 on EDS lag during
  incremental updates (Rationale 2).

## Re-evaluation Triggers

Re-open (as a `tasks/todo/` feature, not a review fix) only if:

- A production incident shows the runtime cross-kind push window causing a measured,
  non-transient outage that multi-replica + readiness probes cannot cover.
- The conf-sync protocol moves to a single multiplexed stream with server-controlled
  ordering, at which point a shared epoch becomes cheap to enforce.
- The project adds a cross-kind config-generation/epoch model for another reason,
  making atomic push an incremental rather than greenfield change.

## Reference Cases

- `tasks/review-envoy/controller-04-no-cross-kind-snapshot-atomicity.md` (closed 2026-06-14, ⚠️ accepted-tradeoff)
- `edgion-controller/src/conf_sync/cache_server/cache.rs` (`apply_change` → `notify_waiters`, per-kind `next_resource_version`)
- `edgion-controller/src/conf_sync/conf_server/config_sync_server.rs` (per-kind `ServerCache<T>`)
- [[cross-kind-dependency-ordering-not-enforced]] (sync-02 — gateway/watch-side mirror; same tradeoff, push-epoch rejected)
- [[partial-update-inconsistency-windows]] (runtime F-005 — cross-store rebuild ordering, same self-heal rationale)
- [[gateway-no-local-config-snapshot]] (sync-05 — sibling Envoy-comparison tradeoff)
- [[listener-port-set-frozen-at-startup]] (listener-01 — sibling startup/Envoy-comparison limitation)
- [[cross-kind-config-not-atomic-extensionref-torn-state]] (resource-02 — sibling: HTTPRoute→EdgionPlugins ExtensionRef torn-state special case)
