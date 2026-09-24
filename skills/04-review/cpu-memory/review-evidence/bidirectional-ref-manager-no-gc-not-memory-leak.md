---
name: bidirectional-ref-manager-no-gc-not-memory-leak
description: Use when reviewing findings that flag BidirectionalRefManager, SecretRefManager, ServiceRefManager, or CrossNamespaceRefManager for lacking GC/expiry methods as a memory leak risk.
knowledge_type: review-evidence
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# `BidirectionalRefManager` Lacking GC / Expiry Mechanism is Not a Memory Leak

**False-positive scenario**: the audit flags `BidirectionalRefManager` in `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/ref_manager.rs` (forward `source_key → Set<V>` + reverse `value_key → Set<source_key>`) for not having `gc` / `cleanup` / `purge` / `expire` methods; "if delete events are lost or processing fails, refs will linger forever"; suggests adding a `gc(existing_values)` periodic task to validate. Typical examples: `SecretRefManager`, `ServiceRefManager`, `CrossNamespaceRefManager`, `BackendTLSPolicyTargetManager`, `stream_plugin_ref_mgr`.

**Reality**:

1. **It is not a cache; it is an index of the current K8s resource graph**: entry lifetime fully follows K8s resources; there is no independent TTL / expiry semantic. Treating it as a generic cache and applying "must have GC" is a category error.

2. **Each reference-producing handler must follow the clear-then-readd pattern**: every `process_*` path first calls `manager.clear_value_refs(&resource_ref)` then loops `add_ref(...)`. Audited examples include `handlers/{http,grpc,tls,tcp,udp}_route.rs`, `edgion_tls.rs`, `edgion_acme.rs`, `edgion_plugins.rs`, `backend_tls_policy.rs`, `gateway.rs`, `link_sys.rs`, and `route_utils.rs`. Even if some event is missed in the middle, the next reconcile of that resource naturally clears stale refs. Do not infer this invariant from the manager itself: verify every handler that starts producing a new reference type.

3. **Each reference-producing handler's delete path must also call `clear_value_refs`**: it is the required pair to the process-path cleanup and must be checked independently.

4. **Watch missed events have a backstop**: Kubernetes informer has built-in periodic resync; watch reconnection replays missed delete events as DELETED; the handler runs clear again.

5. **Leader switchover / Controller restart fully resets**: `runtime.rs reset()` explicitly does `secret_ref_manager.clear()` + `service_ref_mgr.clear()` + `cross_ns_ref_mgr.clear()` + `processor_registry.clear_processors()`; init full-sync rebuilds the index from K8s. Any "long-term accumulated residue" goes to zero on restart.

6. **Memory is bounded**: total reference count = Σ(refs held by each K8s resource); directly bounded by resource graph size. `Set<V>` has built-in dedup (`add_ref` is idempotent); `remove_ref` / `clear_value_refs` also `HashMap::remove`s the bucket after the set is empty (`ref_manager.rs:216-218,222-224,268-271`) — no unbounded growth path.

7. **The suggested `gc(existing_values)` periodic task has side effects**: reconcile is two-phase clear-then-readd; when GC is concurrent with reconcile, it might delete a "currently exists but temporarily not in the index" value in the window between clear and add, instead introducing a bug. Even adding sync primitives is paying lock overhead and reconcile coordination complexity for a non-existent problem.

**Verdict**: not a memory issue. Any "`BidirectionalRefManager` / `SecretRefManager` / `ServiceRefManager` / `CrossNamespaceRefManager` should add GC / expiry / periodic check" finding is closed per this entry, unless it is proved that (a) some handler is missing the `clear_source_refs` / `clear_value_refs` call on the delete path (a per-case bug; fix that location rather than add GC), or (b) under a long stable state, `stats().total_references` measurably grows monotonically and is significantly out of sync with the actual K8s resource count.

**Known fixed cases under (a)**:
- **c2a-007 (fixed)**: `SecretHandler::on_delete` did not purge the forward `secret_key → refs` entries in `SecretRefManager`. Fixed by adding `BidirectionalRefManager::clear_source_refs` and calling it in `on_delete` after the cascading requeue. `SecretRefManager` was also moved from per-processor `Arc` into `ControllerRuntime` (consistent with `service_ref_mgr` / `cross_ns_ref_mgr`) so that (i) `reset()` now correctly clears it, and (ii) its stats are visible in `/api/v1/debug/store-stats` for `cache_residue` leak detection.
- **EdgionGatewayConfig cross-namespace outbound CA references (fixed 2026-09-12)**: `EdgionGatewayConfigHandler` began registering `validation.caCertificateRefs` in `cross_ns_ref_mgr`, but its update and delete paths initially cleared only Secret and global-plugin references. Fixed by clearing `cross_ns_ref_mgr` before each parse and during delete, with a unit test covering target-namespace replacement and deletion. This is a concrete example of exception (a), not evidence for periodic GC.
