---
name: review-concurrency
description: Review knowledge for concurrency, grouped by decision type.
---

# concurrency review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [DSL `run_upstream_response_filter` `block_in_place` Requires Multi-Thread Runtime — DESIGN-TRADEOFF (documented)](intentional-design/block-in-place-multi-thread-runtime.md)
- [Etcd LinkSys health_probe_timeout — request:0 Zero-Duration Timeout (GIGO)](intentional-design/etcd-health-probe-timeout-zero-request-gigo.md)
- [etcd Leadership Gate Cannot Fence a Stale Leader at the Transaction Boundary — ACCEPTED-RISK](intentional-design/etcd-leadership-gate-not-fenced-at-txn.md)
- [`CacheData::reset()` dispatches `full_set` under the write lock — DESIGN-TRADEOFF](intentional-design/full-set-under-write-lock-intentional.md)
- [`LeaderHandle` 100–200ms Polling is Not Worth Replacing With watch::channel](intentional-design/leader-handle-polling.md)
- [Per-Write `is_leader()` Status Guard Is Not a Bug vs client-go Lifecycle Gate — ACCEPTED-TRADEOFF](intentional-design/leader-status-write-per-write-guard-vs-lifecycle-gate.md)
- [Holding a Write Lock Across `metrics` Calls — Style Concern, Not a Latency Bug](intentional-design/lock-around-atomic-metrics.md)
- [`CacheData::set_conf_processor` background debounce task — `JoinHandle` intentionally discarded, no double-call guard](intentional-design/orphaned-bg-task-accepted.md)
- [runtime F-005 — cross-store rebuild ordering in `partial_update` — DESIGN-TRADEOFF](intentional-design/partial-update-inconsistency-windows.md)
- [`CacheData` write lock kept across `partial_update` — DESIGN-TRADEOFF](intentional-design/partial-update-under-lock-intentional.md)
- [`ClientCache::start_watch` keeps backoff+jitter on every watch-break relist — DESIGN-TRADEOFF](intentional-design/relist-backoff-on-server-switch-intentional.md)
- [Controller-Reload Detection via 1s Polling — DESIGN-TRADEOFF](intentional-design/reload-detection-polling-vs-event-driven.md)
- [`ClientCache::start_watch` two remaining extraction phases deliberately deferred — DEFERRED-REFACTOR](intentional-design/start-watch-extract-fn-deferred.md)
- [Watcher `send(...).await` end-to-end backpressure (no `send_timeout`) — DESIGN-TRADEOFF](intentional-design/watcher-send-await-backpressure-no-timeout.md)
- [Webhook active health task is abort-only (no cooperative watch signal) — DESIGN-TRADEOFF](intentional-design/webhook-active-health-abort-only.md)
- [WebhookManager keeps `RwLock` (not `ArcSwap`) on the read path — DESIGN-TRADEOFF](intentional-design/webhook-manager-rwlock-intentional.md)

## Regression guards

- [AsyncRwLock Write Guard Held Across an RPC — Clone-and-Drop, Don't Re-derive](regression-guards/async-write-lock-across-rpc.md)
- [LinkSys ES bulk_ingest_loop Empty endpoints Panic](regression-guards/es-bulk-ingest-empty-endpoints-fail-closed.md)
- [LinkSys Elasticsearch Bulk item-level 429 retry](regression-guards/es-bulk-item-level-429-retry.md)
- [LinkSys Elasticsearch bulk `max_body_bytes` enforcement (link-sys-F002)](regression-guards/es-bulk-max-body-bytes-enforced.md)
- [LinkSys Elasticsearch bulk-ingest shutdown cancellation (link-sys-F016)](regression-guards/es-bulk-shutdown-cancellation.md)
- [link-sys F-012 (etcd) — health-monitor `JoinHandle` is now stored and aborted on shutdown](regression-guards/etcd-monitor-handle-not-aborted.md)
- [Etcd LinkSys start_health_monitor Reconnect Backoff No Jitter (Thundering Herd)](regression-guards/etcd-reconnect-backoff-jitter.md)
- [Failover / EdgionConfigData write TOCTOU fixed with a shared per-resource lock (fr-capi-01, fr-capi-02)](regression-guards/failover-read-modify-write-per-key-lock.md)
- [FileSystem conditional delete validates before it destroys — FIXED](regression-guards/fs-conditional-delete-validate-before-unlink.md)
- [gRPC watch pipeline `None => break` arms now log before breaking (Fix 04)](regression-guards/grpc-watch-none-arm-silent-break.md)
- [link-sys F-B02 — `should_halfopen_probe` admits exactly one probe per window via CAS](regression-guards/halfopen-probe-toctou-cas.md)
- [Leader Lease Never Released on Shutdown — FIXED (best-effort release in the shutdown branch)](regression-guards/leader-lease-release-on-shutdown.md)
- [Leader Pod Label Fire-and-Forget Desync — FIXED (tick-based resync)](regression-guards/leader-pod-label-tick-resync.md)
- [link-sys F-001 (runtime store) — `ProviderRuntimeStore::{insert,remove}` are atomic via `ArcSwap::rcu`](regression-guards/link-sys-runtime-insert-remove-rcu.md)
- [runtime F-013 — `ListenerGatewayInfoStore` publishes flat + index via a single `ArcSwap` snapshot](regression-guards/listener-gateway-info-dual-arcswap-merged-snapshot.md)
- [LinkSys local-file LocalFileWriter Size-Rotation Timer-Gate Overshoot](regression-guards/local-file-per-batch-size-rotation.md)
- [OnceLock init: keep `.set()` outside `debug_assert!`](regression-guards/oncelock-debug-assert-pitfall.md)
- [Body-Buffer Capture Cancellation — FIXED (session poison flag, fail closed)](regression-guards/pingora-body-buffer-capture-cancel-poison.md)
- [H2 Bodyless Replay Registration Requires an Ended Downstream Stream — FIXED](regression-guards/pingora-body-buffer-h2-bodyless-end-stream-required.md)
- [H2 END_STREAM Derived From Original Payload, Not Replay Source — FIXED (registered-buffer guard in `is_body_empty`)](regression-guards/pingora-body-buffer-h2-end-stream-replay-source.md)
- [Body-Buffer Replay `next_chunk` Cancellation Safety — FIXED (two-phase peek/consume)](regression-guards/pingora-body-buffer-replay-cancel-safety.md)
- [Provider cooldown survives a concurrent success](regression-guards/provider-cooldown-survives-concurrent-success.md)
- [Outbound rate-limit atomic window/admission, monotonic expiry and delayed calls](regression-guards/sliding-window-check-then-increment.md)
- [Wasm tick-callout grant registry: evict/register race can drop a fresh grant — ACCEPTED-RISK](regression-guards/wasm-tick-callout-grant-evict-race.md)
- [LinkSys Webhook dispatch credential gate (link-sys-F003)](regression-guards/webhook-dispatch-credential-gate.md)
- [LinkSys webhook `webhook_call` full response-body read before size check (link-sys-F019)](regression-guards/webhook-full-body-read-before-size-check.md)

## Review evidence

- [`conf_sync` watch handshake IS bounded by the endpoint-level timeout (h2-grpc-06)](review-evidence/conf-sync-watch-endpoint-timeout.md)
- [Common LB Module False-Positive Scenarios](review-evidence/lb-false-positives.md)
- [RwLock `.unwrap()` Review Decision Rule](review-evidence/rwlock-unwrap-rule.md)

## Superseded decisions

- [LinkSys webhook `extract_ready_ips` IPv6 bracketing (link-sys-F001, controller path)](superseded/webhook-extract-ready-ips-ipv6-brackets.md)
