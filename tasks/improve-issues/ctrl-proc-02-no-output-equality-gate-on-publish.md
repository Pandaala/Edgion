# ctrl-proc-02: Dependency hooks re-fire on every reconcile, and loop safety relies on a TriggerChain that silently drops requeues

- **Category**: best-practice
- **Severity**: P2
- **Scope**: large-direction
- **Location**:
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/processor.rs:889-926` (on_change → save → after_publish)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/processor.rs:519-522` (`save` → `apply_change(EventUpdate)`)
  - `edgion-controller/src/conf_sync/cache_server/cache.rs:252-270` (`push_event`, no comparison)
  - `edgion-controller/src/conf_mgr/sync_runtime/workqueue.rs:20-80` (TriggerChain cycle/depth limits)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/context.rs` (`HandlerContext::requeue` drops requeues)

## Summary
`ResourceProcessor::process_resource_inner` always calls `on_change`, `save` and `after_publish` for a
successfully parsed object. `ServerCache::push_event` always appends a new sync-version event that all
connected Gateways receive. Nothing checks whether the published (transport) object actually differs from
the cached one. Every requeue therefore becomes a data-plane push plus another round of dependent requeues.
Loop termination depends on hand-written per-edge change guards, with the TriggerChain depth/cycle limiter
as the backstop. When that limiter trips, it silently drops requeues, and nothing periodically resyncs
afterwards.

## Current behavior (evidence)
- `processor.rs:889-893`: `if publish { self.handler.on_change(&parsed_obj, ctx).await; }`. This runs
  unconditionally, and `on_change` is where every Route requeues its parent Gateways and backend policies.
- `processor.rs:917`: `self.save(parsed_obj, None)` → `cache.apply_change(EventUpdate, …)` →
  `push_event` (`cache.rs:252-270`). That appends an event and calls `notify_waiters()` with no equality
  check against the stored object.
- `processor.rs:922-933`: `after_publish` and `notify_policy_ancestor_change` also run on every reconcile.
- Only status persistence is gated (`status_has_changed`, `processor.rs:863-868`). The configuration
  publication path is not.
- Loop safety therefore relies on per-edge guards (`GatewayRouteIndex::update_listener_config_and_collect_routes`,
  tracker diffs, …). If those guards miss, the `TriggerChain` check in `HandlerContext::requeue` drops the
  requeue (`max_trigger_cycles=5`, `max_trigger_depth=20`, `workqueue.rs:180-188`) with only an `error!`
  log. The Kubernetes, FS and etcd centers run no periodic resync (`grep resync conf_mgr/conf_center`
  matches only the etcd compaction resync), so a dropped requeue leaves the dependent stale until an
  unrelated event arrives.
- Concrete consequence: ctrl-proc-01 (endpoint churn → Route/Gateway/EdgionTls republish). The same holds
  for post-init revalidation, leader-change `requeue_all`, and ReferenceGrant namespace-wide
  revalidation. Each re-pushes every touched object even when nothing changed.

## Industry comparison
- **controller-runtime / client-go**: reconcilers are level-triggered and idempotent. Writes go through
  `Patch` with semantic-equality checks (`equality.Semantic.DeepEqual`), so an unchanged object generates
  no watch event and there are no feedback loops. No "trigger chain" exists.
- **Istio pilot**: `PushContext` + debounce. Each xDS resource is hashed and compared per proxy before
  sending (`pushXds` skips unchanged resources in delta mode), and `ConfigUpdate` with no effective change
  produces no push.
- **Envoy Gateway / Contour**: the translator output is compared with the previous snapshot. The xDS
  snapshot cache sends only resources whose version changed.

## Impact
- Data-plane churn and conf_sync ring-buffer wraps (TooOldVersion → full relist) proportional to Controller
  requeue volume rather than to real configuration change.
- Fragile termination: each new requeue edge needs its own bespoke change guard. The TriggerChain limiter
  converts a missed guard into silent staleness instead of a bounded no-op.
- The doc `06-requeue-mechanism.md` must explain cycle safety edge by edge, which is a symptom of this
  design.

## Suggested direction
Options:
1. **Equality gate in the processor (recommended).** After `finalize_transport`, compare the transport
   form with the cached object (`PartialEq` on the typed object, or a stable hash computed once over
   the serialized transport form, which the processor already serializes several times). When it is
   identical and `status_changed == false`, skip `save`/`push_event` and skip `on_change`/`after_publish`
   cascades, while still performing index maintenance that is idempotent. Handlers that intentionally
   force a republish (none should need to once ctrl-proc-01 item 3 lands) can opt out explicitly.
2. A cheaper alternative is to gate only the conf_sync push (`ServerCache::apply_change` drops an
   `EventUpdate` whose payload equals the stored one) and leave hooks unchanged. This fixes data-plane
   churn but not requeue amplification.

With option 1 in place, TriggerChain can shrink to a diagnostic (log chain depth) instead of a correctness
mechanism that drops work. If any limit remains, a tripped limit should requeue with backoff rather than
drop.

## Notes
- **Overlap:** the (dropped) ctrl-center unconditional-republish draft covers the conf_sync *publication* half
  (gate in `EventStore::apply_event`). This finding covers the other half, which a store-level gate cannot
  fix: `on_change`/`after_publish` requeue cascades still fire on no-op reconciles, and the TriggerChain
  drop-without-resync behavior remains. Implement the two together, with the gate at the processor so that
  one comparison suppresses both.
- Checked `skills/04-review/conf-sync/intentional-design/ring-buffer-fixed-capacity-toooldversion-relist.md`.
  It accepts the ring buffer + relist tradeoff given real churn. The new evidence here is that a large share
  of the churn is self-inflicted no-op republishing, which that page does not consider.
- Checked `skills/04-review/cpu-memory/review-evidence/extract-status-json-double-serialization-cold-path.md`
  and `small-object-clones-reconcile-config-sync.md`. Those pages treat per-reconcile serialization CPU as
  cold-path. This finding is about fan-out and data-plane pushes, not local CPU.
- `skills/01-architecture/01-controller/06-requeue-mechanism.md` "Cycle Safety" documents the per-edge
  guard approach. This finding proposes replacing it with a generic gate.

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P2
- `edgion-controller/src/conf_sync/cache_server/cache.rs:252-270`: `push_event` always calls `apply_event` + `notify_waiters()` with no comparison.
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/context.rs:95-117`: depth/cycle limit → `tracing::error!` and `return` (requeue dropped).
- `edgion-controller/src/conf_mgr/sync_runtime/workqueue.rs:181-188`: defaults `max_trigger_cycles: 5`, `max_trigger_depth: 20`.
- Distinct from ctrl-proc-01 (generic mechanism vs specific chain); keep both, implement together.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2 (unchanged)
- `resource_processor/processor.rs`, `context.rs`, `workqueue.rs` and `conf_sync/cache_server/cache.rs` are unchanged since `feature-08-30`.
- `cache.rs:252-270`: `push_event` still appends with no equality check; `context.rs:95-117` still drops requeues on TriggerChain limits with only `error!`.
