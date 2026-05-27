---
name: cpu-memory-review
description: Use when reviewing CPU or memory performance findings; index of per-scenario decision rules for accepted patterns (lazy cleanup, mandatory .to_string() API, single-shot allocations, etc.).
---

# CPU / Memory Performance Review

## Files in this directory

| File | Purpose |
|------|---------|
| [performance-review-methodology.md](performance-review-methodology.md) | General decision methodology — trace call chain, distinguish phases, verify mechanism |
| [not-a-performance-issue.md](not-a-performance-issue.md) | Legacy stub — contents split into `cases/`; maintained for link compatibility |
| [cases/](cases/) | Individual per-scenario closed-case files (one decision rule per file) |

## Quick routing

- **Is this a general "is it a performance issue?" question?** → [performance-review-methodology.md](performance-review-methodology.md)
- **Looking for a specific named scenario?** → see the case table below or search `cases/`
- **Checking a finding against a known accepted tradeoff?** → find the matching row and load that file

See also [../CLOSED-FINDINGS.md](../CLOSED-FINDINGS.md) for the cross-topic closure log.

## Case Index

| File | Description |
|------|-------------|
| [`cases/metrics-label-to-string-api.md`](cases/metrics-label-to-string-api.md) | `.to_string()` in metrics recording functions — mandatory for `Cow<'static, str>` API |
| [`cases/control-plane-status-write-counter-pre-caching.md`](cases/control-plane-status-write-counter-pre-caching.md) | Pre-caching Counter handles for `record_status_write` on controller plane — not worth it |
| [`cases/get-raw-any-lookup-not-json-scan.md`](cases/get-raw-any-lookup-not-json-scan.md) | `get_raw_any` / `lookup_gateway` is O(1) DashMap lookup, not JSON serialize + scan |
| [`cases/http-grpc-route-regex-matching-no-fallback.md`](cases/http-grpc-route-regex-matching-no-fallback.md) | Hot-path `Regex::new()` fallback in `match_header` / `match_query_param` — removed; no re-add |
| [`cases/watcher-vec-lazy-cleanup.md`](cases/watcher-vec-lazy-cleanup.md) | `RwLock<Vec<WatchClient>>` lazy cleanup — sufficient; bounded < 20KB |
| [`cases/webhook-read-then-truncate-16kb-cap.md`](cases/webhook-read-then-truncate-16kb-cap.md) | `read_body_limited` 1GB buffering claim — actual hard cap is 16KB |
| [`cases/oncelock-global-singleton-deliberate-design.md`](cases/oncelock-global-singleton-deliberate-design.md) | `GLOBAL_CONFIG_CLIENT OnceLock` not releasing on reconnect — deliberate design |
| [`cases/small-object-clones-reconcile-config-sync.md`](cases/small-object-clones-reconcile-config-sync.md) | Small-object clones / `config.clone()` on reconcile / config-sync paths — not per-request |
| [`cases/oidc-jwt-claims-json-serialization-opt-in.md`](cases/oidc-jwt-claims-json-serialization-opt-in.md) | OIDC/JWT claims `serde_json::to_string` per request — serialization is the opt-in feature itself |
| [`cases/offline-controller-data-retention-business-requirement.md`](cases/offline-controller-data-retention-business-requirement.md) | Offline controller entries not GC'd in `ControllerRegistry` — business-semantics retention |
| [`cases/extract-status-json-double-serialization-cold-path.md`](cases/extract-status-json-double-serialization-cold-path.md) | `extract_status_json` called twice in `process_resource_inner` — cold path + API shape |
| [`cases/config-struct-field-ordering-padding-waste.md`](cases/config-struct-field-ordering-padding-waste.md) | Config struct field reorder for padding — field order is part of user-visible JSON schema |
| [`cases/hashmap-new-default-capacity-reconcile-paths.md`](cases/hashmap-new-default-capacity-reconcile-paths.md) | `HashMap::new()` without `with_capacity` on `full_set` / reconcile paths — cold event-level, tiny scale |
| [`cases/access-log-json-serialization-per-request.md`](cases/access-log-json-serialization-per-request.md) | `AccessLogEntry::to_json()` per-request String alloc — channel boundary shape requires owned String |
| [`cases/centerdb-single-mutex-connection-not-blocking.md`](cases/centerdb-single-mutex-connection-not-blocking.md) | `CenterDb` single `Mutex<Connection>` — all ops are event-level and run in `spawn_blocking` |
| [`cases/admin-api-controller-summaries-multi-call.md`](cases/admin-api-controller-summaries-multi-call.md) | `controller_summaries()` multi-call with partial field use — admin/ops path, human scale |
| [`cases/watch-cache-registry-get-or-create-slow-path.md`](cases/watch-cache-registry-get-or-create-slow-path.md) | `controller_id.to_string()` twice in `get_or_create` write-lock branch — once per controller lifetime |
| [`cases/record-cross-ns-refs-write-lock-per-item.md`](cases/record-cross-ns-refs-write-lock-per-item.md) | `record_cross_ns_refs` one write lock per `backend_ref` — cold reconcile path, < 2.5 µs total |
| [`cases/bidirectional-ref-manager-no-gc-not-memory-leak.md`](cases/bidirectional-ref-manager-no-gc-not-memory-leak.md) | `BidirectionalRefManager` / `SecretRefManager` lacking GC — it is an index, not a cache |
| [`cases/workqueue-delay-heap-no-size-cap.md`](cases/workqueue-delay-heap-no-size-cap.md) | Workqueue `BinaryHeap<DelayedItem>` no explicit size cap — dedup by key + per-Kind bounds |
| [`cases/conf-sync-watcher-mpsc-chain-not-backlog.md`](cases/conf-sync-watcher-mpsc-chain-not-backlog.md) | conf_sync watcher `mpsc(100)` × 3 layers — items are batches, not single events; end-to-end backpressure |
| [`cases/status-diff-conditions-equal-on2-bounded.md`](cases/status-diff-conditions-equal-on2-bounded.md) | `conditions_equal` O(n²) scan — n ≤ 10; HashMap overhead exceeds linear scan benefit |
| [`cases/admin-state-cache-get-resource-full-list-scan.md`](cases/admin-state-cache-get-resource-full-list-scan.md) | `cache_get_resource` full-list + linear-scan — admin/diagnostic path, bounded by human ops |
| [`cases/conf-sync-compress-event-vec-per-key.md`](cases/conf-sync-compress-event-vec-per-key.md) | `CompressEvent` per-key `Vec<ResourceChange>` "unbounded / OOM / O(N) scan" — confirmed-real but won't-fix; control-plane, 1-byte enum, trivially bounded |
| [`cases/udp-established-session-route-pipeline-per-packet.md`](cases/udp-established-session-route-pipeline-per-packet.md) | UDP `handle_client_packet` ran full route pipeline per packet for established sessions — FIXED (F-016) with a `client_sessions` fast-path |
| [`cases/dsl-step-budget-not-cpu-bound.md`](cases/dsl-step-budget-not-cpu-bound.md) | DSL VM `max_steps` counts opcode dispatches, not CPU time — per-opcode CPU is bounded by `max_string_len` + per-regex DFA limit + per-builtin output caps; the four-cap product is the runtime CPU bound |
| [`cases/gcir-stream-plugin-store-key-precompute.md`](cases/gcir-stream-plugin-store-key-precompute.md) | `GlobalConnectionIpRestrictionPlugin` per-connection `format!("{}/{}", ns, name)` in PM-store lookup — FIXED (F-008) by precomputing the key in `new()` and calling the `_by_key` getter on the accept path |
