---
name: review-cpu-memory
description: Review knowledge for cpu-memory, grouped by decision type.
---

# cpu-memory review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [HTTP request context layout budget](intentional-design/http-request-context-layout-budget.md)

- [Access-log raw URI ownership and optimization boundary](intentional-design/access-log-request-uri-include-raw-uri-alloc.md)
- [`AdminState::cache_get_resource` Full-List + Linear-Scan is Not a Performance Issue](intentional-design/admin-state-cache-get-resource-full-list-scan.md)
- [Async response-body hook no-work gate requires production profiling](intentional-design/async-response-body-hook-profile-before-gate.md)
- [Backend generation handles retain the immutable slice layout](intentional-design/backend-generation-arc-slice-handle-layout.md)
- [conf_sync `CompressEvent` Per-Key `Vec<ResourceChange>` — DESIGN-TRADEOFF (won't-fix)](intentional-design/conf-sync-compress-event-vec-per-key.md)
- [Configuration Struct Field Ordering Causing Padding Waste](intentional-design/config-struct-field-ordering-padding-waste.md)
- [Control-Plane Status-Write Counter Handle Pre-Caching is Not Worth It](intentional-design/control-plane-status-write-counter-pre-caching.md)
- [DSL VM step budget counts opcode dispatches, not CPU time (intentional)](intentional-design/dsl-step-budget-not-cpu-bound.md)
- [HTTP/1 Captured Request-Body Second Copy — ACCEPTED TRADEOFF](intentional-design/h1-request-body-capture-bounded-second-copy.md)
- [`invalid_rule_indices` `Vec::contains` Linear Scan in HTTP/gRPC Route Compile Loops is Not an Issue](intentional-design/invalid-rule-indices-vec-contains-config-load.md)
- [OnceLock Global Singleton is Deliberate Design](intentional-design/oncelock-global-singleton-deliberate-design.md)
- [Wasm shared data: no eviction, no cluster KV backend, process-local lifetime — DESIGN-TRADEOFF](intentional-design/wasm-shared-data-no-eviction-no-cluster-kv.md)

## Compatibility constraints

- [metrics label `.to_string()` is an API Hard Requirement](compatibility-constraints/metrics-label-to-string-api.md)

## Regression guards

- [Response-body conditions borrow live inputs for each evaluation](regression-guards/response-condition-live-input-borrowing.md)
- [Canonical outbound clients bound decoded HTTP/2 response headers](regression-guards/outbound-http2-response-header-list-bound.md)
- [Adaptive telemetry reconciliation is route-contribution incremental](regression-guards/adaptive-telemetry-reconcile-is-incremental.md)
- [AI runtime stores must report cache residue](regression-guards/ai-runtime-stores-must-report-residue.md)
- [`edgion-cli` Outbound Client Idle Pool Disabled — FIXED (EDG-CLI-002)](regression-guards/cli-http-client-idle-pool-disabled.md)
- [Controller ConfCenter stats use the canonical resource inventory](regression-guards/controller-confcenter-stats-canonical-inventory.md)
- [`DomainRouteRules` Inner `ArcSwap` Fields Removed (REAL FIX)](regression-guards/domain-route-rules-inner-arcswap-removed.md)
- [`url_encode` builds incrementally and bails on overflow (fixed)](regression-guards/dsl-url-encode-streaming-bail.md)
- [`GlobalConnectionIpRestrictionPlugin` Per-Connection Store-Key `format!` — FIXED (F-008)](regression-guards/gcir-stream-plugin-store-key-precompute.md)
- [Gateway identity keys are precomputed per shared generation](regression-guards/gateway-identity-key-precomputation.md)
- [Header-only HMAC condition verifier is prebuilt — FIXED (cl-05)](regression-guards/header-hmac-condition-verifier-prebuilt.md)
- [Local Response Bodies Are Clone-cheap Immutable `Bytes` — FIXED](regression-guards/local-response-body-is-clone-cheap-bytes.md)
- [LRU Expiration Maintenance Is a Bounded Ordered Index — FIXED](regression-guards/lru-bounded-expiration-index.md)
- [OIDC Access-Token Cache Stores Expiry Only as TTL — FIXED](regression-guards/oidc-access-token-cache-ttl-only.md)
- [OIDC callback path is precomputed — FIXED (gw-plugin-oidc-01)](regression-guards/oidc-callback-path-precomputed.md)
- [Outlier ejection arbitrates only threshold candidates](regression-guards/outlier-ejection-arbitrates-only-threshold-candidates.md)
- [`RequestMirrorPlugin` `reqwest::Client` Per-Plugin Instance — FIXED (F-017)](regression-guards/request-mirror-reqwest-client-per-plugin-instance.md)
- [`RouteRules` Inner `RwLock` Fields Removed (REAL FIX)](regression-guards/route-rules-inner-rwlock-fields-removed.md)
- [`record_backend_request` / `record_request_duration` per-request allocations](regression-guards/route-unit-metric-handles.md)
- [`EdgionHttpProxy` Caches Per-Listener Route Manager `Arc`s (REAL FIX)](regression-guards/routes-http-listener-manager-per-request-lookup.md)
- [`TcpLogEntry::from_context` Per-Connection `format!("{:?}", TcpStatus)` — FIXED (observe-F012)](regression-guards/tcp-log-entry-status-debug-format-replaced.md)
- [UDP `handle_client_packet` Per-Packet Route Pipeline for Established Sessions — FIXED (F-016)](regression-guards/udp-established-session-route-pipeline-per-packet.md)
- [Wasm guest amplifying host memory via unbounded retained ABI state — FIXED](regression-guards/wasm-abi-retention-budget.md)
- [Wasm artifact response bodies buffered before the size check — FIXED (streaming cap)](regression-guards/wasm-artifact-response-streaming-cap.md)

## Review evidence

- [Access Log JSON Serialization Allocating a New String per Request is Not an Issue](review-evidence/access-log-json-serialization-per-request.md)
- [`BidirectionalRefManager` Lacking GC / Expiry Mechanism is Not a Memory Leak](review-evidence/bidirectional-ref-manager-no-gc-not-memory-leak.md)
- [conf_sync Watcher Queue Backpressure and Payload Retention](review-evidence/conf-sync-watcher-mpsc-chain-not-backlog.md)
- [`extract_status_json` Double Serialization is Cold Path + API Shape](review-evidence/extract-status-json-double-serialization-cold-path.md)
- [`get_raw_any` / `lookup_gateway` is Not JSON Serialization](review-evidence/get-raw-any-lookup-not-json-scan.md)
- [`HashMap::new()` Default Capacity on Reconcile / Full-Sync Paths](review-evidence/hashmap-new-default-capacity-reconcile-paths.md)
- [HTTP/gRPC Route Regex Matching: Controller Validates + No Fallback on Hot Path](review-evidence/http-grpc-route-regex-matching-no-fallback.md)
- [Non-Memory-Leak Scenarios](review-evidence/not-a-memory-leak.md)
- [OIDC / JWT Claims JSON Serialization is the Opt-In Feature Itself](review-evidence/oidc-jwt-claims-json-serialization-opt-in.md)
- [CPU/Memory Performance Audit Methodology](review-evidence/performance-review-methodology.md)
- [Route Handlers `record_cross_ns_refs` Loop `add_ref` is Not a Lock Hotspot](review-evidence/record-cross-ns-refs-write-lock-per-item.md)
- [Small-Object Clones on Resource Reconcile / Config Sync Paths Are Not Issues](review-evidence/small-object-clones-reconcile-config-sync.md)
- [`status_diff::conditions_equal` O(n²) Linear Scan on Bounded Condition Arrays is Not an Issue](review-evidence/status-diff-conditions-equal-on2-bounded.md)
- [Watcher Vec Lazy Cleanup is Sufficient](review-evidence/watcher-vec-lazy-cleanup.md)
- [Webhook response buffering: streaming-cap evidence](review-evidence/webhook-read-then-truncate-16kb-cap.md)
- [Workqueue Delay Heap Without Explicit Size Cap is Not an Issue](review-evidence/workqueue-delay-heap-no-size-cap.md)

## Superseded decisions

- [Historical Wasm OCI disk-purge interval clamp](superseded/wasm-oci-purge-interval-zero-clamp.md)
- [`IncludeCondition` / `ExcludeCondition` HashSet/Wildcard Bucket Split (plugins-runtime-F005)](superseded/conditions-hashset-wildcard-bucket-split.md)
