# Gateway File Inventory

## Frozen baseline

- Baseline commit: `d484e580c8274ce58426c9fe6729e444f505bdaf`
- Baseline Gateway tracked files: 530
- Baseline integration-test tree: `ae68dedd863020ddd6d035d7404570716429ecd1`
- Existing integration-test files may not be modified, renamed, or deleted. Additions only.
- A batch closes only when every row assigned to it leaves `pending` and its total reconciles exactly.

## Execution waves

1. Utilities and caches.
2. Protocol logs, access logs/diagnostics, and metrics.
3. Request-body and SSE infrastructure; read `ctx.rs` but defer changing it until consumer context is complete.
4. Configuration, conf-sync, and runtime stores.
5. TLS, backends, and load balancing.
6. LinkSys, GeoIP, and services.
7. Plugin runtime/framework, then AI core and adapters.
8. Leaf HTTP plugins; review JWT and OIDC separately.
9. Complex plugins: ExtProc, DSL, and Wasm.
10. Stream plugins.
11. Routes: shared/gRPC, HTTP matching/manager, Pingora lifecycle, and L4 families.
12. Admin API, CLI/startup, crate roots, tests, and final reconciliation.

## States

- `pending`: not yet completely audited.
- `reviewed-no-change`: complete audit found no worthwhile safe change.
- `optimized`: changed, reviewed, verified, and committed.
- `test-baseline`: audited as a regression specification and intentionally unchanged.
- `excluded`: audited but prohibited or externally generated; requires a reason in Change ID / note.

## Batch coverage

| Batch | Expected | Optimized | No change | Other closed | Status |
|-------|---------:|----------:|----------:|-------------:|--------|
| `ai-anthropic` | 3 | 0 | 0 | 0 | pending |
| `ai-core` | 16 | 0 | 0 | 0 | pending |
| `ai-quota` | 2 | 0 | 0 | 0 | pending |
| `api` | 2 | 0 | 0 | 0 | pending |
| `backends-circuit-breaker` | 2 | 0 | 0 | 0 | pending |
| `backends-core` | 10 | 0 | 0 | 0 | pending |
| `backends-discovery-endpoints` | 4 | 0 | 0 | 0 | pending |
| `backends-discovery-services` | 3 | 0 | 0 | 0 | pending |
| `backends-discovery-shared` | 3 | 0 | 0 | 0 | pending |
| `backends-health` | 6 | 0 | 0 | 0 | pending |
| `backends-outlier` | 4 | 0 | 0 | 0 | pending |
| `backends-policy-root` | 1 | 0 | 0 | 0 | pending |
| `backends-policy-tls` | 3 | 0 | 0 | 0 | pending |
| `backends-policy-traffic` | 5 | 0 | 0 | 0 | pending |
| `cache` | 4 | 2 | 2 | 0 | complete |
| `cli` | 3 | 0 | 0 | 0 | pending |
| `conf-sync-cache-client` | 4 | 0 | 0 | 0 | pending |
| `conf-sync-conf-client` | 5 | 0 | 0 | 0 | pending |
| `conf-sync-root` | 1 | 0 | 0 | 0 | pending |
| `config-edgion-gateway` | 2 | 0 | 0 | 0 | pending |
| `config-gateway-class` | 2 | 0 | 0 | 0 | pending |
| `config-root` | 1 | 0 | 0 | 0 | pending |
| `crate-entry` | 2 | 0 | 0 | 0 | pending |
| `crate-root` | 2 | 0 | 0 | 0 | pending |
| `external-tests` | 1 | 0 | 0 | 0 | pending |
| `geoip` | 5 | 0 | 0 | 0 | pending |
| `http-context` | 1 | 0 | 0 | 0 | pending |
| `lb-backend-selector` | 3 | 0 | 0 | 0 | pending |
| `lb-core` | 4 | 0 | 0 | 0 | pending |
| `lb-ewma` | 2 | 0 | 0 | 0 | pending |
| `lb-leastconn` | 2 | 0 | 0 | 0 | pending |
| `lb-runtime-state` | 1 | 0 | 0 | 0 | pending |
| `lb-selection` | 4 | 0 | 0 | 0 | pending |
| `link-sys-provider-elasticsearch` | 6 | 0 | 0 | 0 | pending |
| `link-sys-provider-kafka` | 3 | 0 | 0 | 0 | pending |
| `link-sys-provider-local-file` | 3 | 0 | 0 | 0 | pending |
| `link-sys-provider-otlp` | 2 | 0 | 0 | 0 | pending |
| `link-sys-provider-redis` | 4 | 0 | 0 | 0 | pending |
| `link-sys-provider-webhook` | 5 | 0 | 0 | 0 | pending |
| `link-sys-providers-root` | 1 | 0 | 0 | 0 | pending |
| `link-sys-root` | 2 | 0 | 0 | 0 | pending |
| `link-sys-runtime` | 5 | 0 | 0 | 0 | pending |
| `link-sys-template` | 2 | 0 | 0 | 0 | pending |
| `observe-access-log` | 5 | 0 | 0 | 0 | pending |
| `observe-core` | 3 | 0 | 0 | 0 | pending |
| `observe-metrics` | 5 | 0 | 0 | 0 | pending |
| `observe-protocol-logs` | 7 | 4 | 3 | 0 | complete |
| `plugins-common` | 3 | 0 | 0 | 0 | pending |
| `plugins-http-ai-guard` | 5 | 0 | 0 | 0 | pending |
| `plugins-http-ai-proxy` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-all-endpoint-status` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-bandwidth-limit` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-basic-auth` | 3 | 0 | 0 | 0 | pending |
| `plugins-http-canary` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-common` | 13 | 0 | 0 | 0 | pending |
| `plugins-http-cors` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-credential-injector` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-csrf` | 3 | 0 | 0 | 0 | pending |
| `plugins-http-ctx-set` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-direct-endpoint` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-dsl` | 33 | 0 | 0 | 0 | pending |
| `plugins-http-dynamic-external-upstream` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-dynamic-internal-upstream` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-ext-proc` | 8 | 0 | 0 | 0 | pending |
| `plugins-http-fault-injection` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-form-json-transform` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-forward-auth` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-framework` | 3 | 0 | 0 | 0 | pending |
| `plugins-http-geoip-location` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-guardrail` | 5 | 0 | 0 | 0 | pending |
| `plugins-http-header-cert-auth` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-hmac-auth` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-ip-restriction` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-json-schema-validation` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-jwe-decrypt` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-jwt-auth` | 15 | 0 | 0 | 0 | pending |
| `plugins-http-key-auth` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-ldap-auth` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-mock` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-openid-connect` | 16 | 1 | 0 | 0 | pending |
| `plugins-http-proxy-rewrite` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-rate-limit` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-rate-limit-redis` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-real-ip` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-region-route` | 3 | 0 | 0 | 0 | pending |
| `plugins-http-request-access-policy` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-request-body-buffer` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-request-mirror` | 4 | 0 | 0 | 0 | pending |
| `plugins-http-request-restriction` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-response-rewrite` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-trace-context` | 3 | 0 | 0 | 0 | pending |
| `plugins-http-waf` | 1 | 0 | 0 | 0 | pending |
| `plugins-http-wasm` | 10 | 0 | 0 | 0 | pending |
| `plugins-http-webhook-key-get` | 2 | 0 | 0 | 0 | pending |
| `plugins-root` | 1 | 0 | 0 | 0 | pending |
| `plugins-runtime` | 33 | 0 | 0 | 0 | pending |
| `plugins-stream` | 18 | 0 | 0 | 0 | pending |
| `request-body` | 9 | 0 | 0 | 0 | pending |
| `routes-shared` | 65 | 0 | 0 | 0 | pending |
| `runtime` | 15 | 0 | 0 | 0 | pending |
| `services` | 4 | 0 | 0 | 0 | pending |
| `sse` | 3 | 0 | 0 | 0 | pending |
| `tls-boringssl` | 2 | 0 | 0 | 0 | pending |
| `tls-core` | 3 | 0 | 0 | 0 | pending |
| `tls-runtime` | 6 | 0 | 0 | 0 | pending |
| `tls-store` | 4 | 0 | 0 | 0 | pending |
| `tls-validation` | 5 | 0 | 0 | 0 | pending |
| `util` | 6 | 4 | 2 | 0 | complete |

## File ledger

| File | Batch | Kind | State | Change ID / note |
|------|-------|------|-------|------------------|
| `edgion-gateway/Cargo.toml` | `crate-root` | manifest | pending | — |
| `edgion-gateway/build.rs` | `crate-root` | source | pending | — |
| `edgion-gateway/src/ai/adaptive.rs` | `ai-core` | source | pending | — |
| `edgion-gateway/src/ai/anthropic/mod.rs` | `ai-anthropic` | source | pending | — |
| `edgion-gateway/src/ai/anthropic/request.rs` | `ai-anthropic` | source | pending | — |
| `edgion-gateway/src/ai/anthropic/response.rs` | `ai-anthropic` | source | pending | — |
| `edgion-gateway/src/ai/attempt.rs` | `ai-core` | source | pending | — |
| `edgion-gateway/src/ai/attempt_plan.rs` | `ai-core` | source | pending | — |
| `edgion-gateway/src/ai/attempt_runtime.rs` | `ai-core` | source | pending | — |
| `edgion-gateway/src/ai/backend_store.rs` | `ai-core` | source | pending | — |
| `edgion-gateway/src/ai/canonical_sse.rs` | `ai-core` | source | pending | — |
| `edgion-gateway/src/ai/circuit_breaker.rs` | `ai-core` | source | pending | — |
| `edgion-gateway/src/ai/credential_outlier.rs` | `ai-core` | source | pending | — |
| `edgion-gateway/src/ai/dispatch.rs` | `ai-core` | source | pending | — |
| `edgion-gateway/src/ai/error.rs` | `ai-core` | source | pending | — |
| `edgion-gateway/src/ai/json_span.rs` | `ai-core` | source | pending | — |
| `edgion-gateway/src/ai/mod.rs` | `ai-core` | source | pending | — |
| `edgion-gateway/src/ai/model_router.rs` | `ai-core` | source | pending | — |
| `edgion-gateway/src/ai/quota/mod.rs` | `ai-quota` | source | pending | — |
| `edgion-gateway/src/ai/quota/runtime.rs` | `ai-quota` | source | pending | — |
| `edgion-gateway/src/ai/routing.rs` | `ai-core` | source | pending | — |
| `edgion-gateway/src/ai/terminal.rs` | `ai-core` | source | pending | — |
| `edgion-gateway/src/ai/usage.rs` | `ai-core` | source | pending | — |
| `edgion-gateway/src/api/admin_auth.rs` | `api` | source | pending | — |
| `edgion-gateway/src/api/mod.rs` | `api` | source | pending | — |
| `edgion-gateway/src/backends/circuit_breaker/concurrency_integration_tests.rs` | `backends-circuit-breaker` | test | pending | — |
| `edgion-gateway/src/backends/circuit_breaker/mod.rs` | `backends-circuit-breaker` | source | pending | — |
| `edgion-gateway/src/backends/discovery/conf_handler_common.rs` | `backends-discovery-shared` | source | pending | — |
| `edgion-gateway/src/backends/discovery/dns_cache.rs` | `backends-discovery-shared` | source | pending | — |
| `edgion-gateway/src/backends/discovery/endpoint_slice/conf_handler_impl.rs` | `backends-discovery-endpoints` | source | pending | — |
| `edgion-gateway/src/backends/discovery/endpoint_slice/discovery_impl.rs` | `backends-discovery-endpoints` | source | pending | — |
| `edgion-gateway/src/backends/discovery/endpoint_slice/ep_slice_store.rs` | `backends-discovery-endpoints` | source | pending | — |
| `edgion-gateway/src/backends/discovery/endpoint_slice/mod.rs` | `backends-discovery-endpoints` | source | pending | — |
| `edgion-gateway/src/backends/discovery/mod.rs` | `backends-discovery-shared` | source | pending | — |
| `edgion-gateway/src/backends/discovery/services/conf_handler_impl.rs` | `backends-discovery-services` | source | pending | — |
| `edgion-gateway/src/backends/discovery/services/mod.rs` | `backends-discovery-services` | source | pending | — |
| `edgion-gateway/src/backends/discovery/services/service_store.rs` | `backends-discovery-services` | source | pending | — |
| `edgion-gateway/src/backends/gateway_backend_tls_store.rs` | `backends-core` | source | pending | — |
| `edgion-gateway/src/backends/health/check/config_store.rs` | `backends-health` | source | pending | — |
| `edgion-gateway/src/backends/health/check/manager.rs` | `backends-health` | source | pending | — |
| `edgion-gateway/src/backends/health/check/mod.rs` | `backends-health` | source | pending | — |
| `edgion-gateway/src/backends/health/check/probes.rs` | `backends-health` | source | pending | — |
| `edgion-gateway/src/backends/health/check/status_store.rs` | `backends-health` | source | pending | — |
| `edgion-gateway/src/backends/health/mod.rs` | `backends-health` | source | pending | — |
| `edgion-gateway/src/backends/mod.rs` | `backends-core` | source | pending | — |
| `edgion-gateway/src/backends/outlier_detection/config.rs` | `backends-outlier` | source | pending | — |
| `edgion-gateway/src/backends/outlier_detection/mod.rs` | `backends-outlier` | source | pending | — |
| `edgion-gateway/src/backends/outlier_detection/state.rs` | `backends-outlier` | source | pending | — |
| `edgion-gateway/src/backends/outlier_detection/store.rs` | `backends-outlier` | source | pending | — |
| `edgion-gateway/src/backends/panic_threshold.rs` | `backends-core` | source | pending | — |
| `edgion-gateway/src/backends/peer_builder.rs` | `backends-core` | source | pending | — |
| `edgion-gateway/src/backends/policy/backend_tls/backend_tls_policy_store.rs` | `backends-policy-tls` | source | pending | — |
| `edgion-gateway/src/backends/policy/backend_tls/conf_handler_impl.rs` | `backends-policy-tls` | source | pending | — |
| `edgion-gateway/src/backends/policy/backend_tls/mod.rs` | `backends-policy-tls` | source | pending | — |
| `edgion-gateway/src/backends/policy/backend_traffic/authority.rs` | `backends-policy-traffic` | source | pending | — |
| `edgion-gateway/src/backends/policy/backend_traffic/conf_handler_impl.rs` | `backends-policy-traffic` | source | pending | — |
| `edgion-gateway/src/backends/policy/backend_traffic/mod.rs` | `backends-policy-traffic` | source | pending | — |
| `edgion-gateway/src/backends/policy/backend_traffic/retry_budget.rs` | `backends-policy-traffic` | source | pending | — |
| `edgion-gateway/src/backends/policy/backend_traffic/store.rs` | `backends-policy-traffic` | source | pending | — |
| `edgion-gateway/src/backends/policy/mod.rs` | `backends-policy-root` | source | pending | — |
| `edgion-gateway/src/backends/port_resolver.rs` | `backends-core` | source | pending | — |
| `edgion-gateway/src/backends/preload.rs` | `backends-core` | source | pending | — |
| `edgion-gateway/src/backends/runtime.rs` | `backends-core` | source | pending | — |
| `edgion-gateway/src/backends/selector.rs` | `backends-core` | source | pending | — |
| `edgion-gateway/src/backends/service_kind.rs` | `backends-core` | source | pending | — |
| `edgion-gateway/src/backends/validation.rs` | `backends-core` | source | pending | — |
| `edgion-gateway/src/bin/edgion_gateway.rs` | `crate-entry` | source | pending | — |
| `edgion-gateway/src/cache/lru.rs` | `cache` | source | optimized | CACHE-001 / `40f7eb74` |
| `edgion-gateway/src/cache/mod.rs` | `cache` | source | reviewed-no-change | CACHE-001 / `40f7eb74` |
| `edgion-gateway/src/cache/sharded.rs` | `cache` | source | optimized | CACHE-001 / `40f7eb74` |
| `edgion-gateway/src/cache/tiny_ufo.rs` | `cache` | source | reviewed-no-change | CACHE-001 / `40f7eb74` |
| `edgion-gateway/src/cli/config.rs` | `cli` | source | pending | — |
| `edgion-gateway/src/cli/mod.rs` | `cli` | source | pending | — |
| `edgion-gateway/src/cli/pingora.rs` | `cli` | source | pending | — |
| `edgion-gateway/src/conf_sync/cache_client/cache.rs` | `conf-sync-cache-client` | source | pending | — |
| `edgion-gateway/src/conf_sync/cache_client/cache_data.rs` | `conf-sync-cache-client` | source | pending | — |
| `edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs` | `conf-sync-cache-client` | source | pending | — |
| `edgion-gateway/src/conf_sync/cache_client/mod.rs` | `conf-sync-cache-client` | source | pending | — |
| `edgion-gateway/src/conf_sync/conf_client/cert_watch.rs` | `conf-sync-conf-client` | source | pending | — |
| `edgion-gateway/src/conf_sync/conf_client/config_client.rs` | `conf-sync-conf-client` | source | pending | — |
| `edgion-gateway/src/conf_sync/conf_client/grpc_client.rs` | `conf-sync-conf-client` | source | pending | — |
| `edgion-gateway/src/conf_sync/conf_client/mod.rs` | `conf-sync-conf-client` | source | pending | — |
| `edgion-gateway/src/conf_sync/conf_client/rotation.rs` | `conf-sync-conf-client` | source | pending | — |
| `edgion-gateway/src/conf_sync/mod.rs` | `conf-sync-root` | source | pending | — |
| `edgion-gateway/src/config/edgion_gateway/conf_handler_impl.rs` | `config-edgion-gateway` | source | pending | — |
| `edgion-gateway/src/config/edgion_gateway/mod.rs` | `config-edgion-gateway` | source | pending | — |
| `edgion-gateway/src/config/gateway_class/conf_handler_impl.rs` | `config-gateway-class` | source | pending | — |
| `edgion-gateway/src/config/gateway_class/mod.rs` | `config-gateway-class` | source | pending | — |
| `edgion-gateway/src/config/mod.rs` | `config-root` | source | pending | — |
| `edgion-gateway/src/ctx.rs` | `http-context` | source | pending | OBS-LOG-001 follow-up: verify `TlsConnMeta.sni` masking |
| `edgion-gateway/src/geoip/config.rs` | `geoip` | source | pending | — |
| `edgion-gateway/src/geoip/info.rs` | `geoip` | source | pending | — |
| `edgion-gateway/src/geoip/mod.rs` | `geoip` | source | pending | — |
| `edgion-gateway/src/geoip/provider.rs` | `geoip` | source | pending | — |
| `edgion-gateway/src/geoip/updater.rs` | `geoip` | source | pending | — |
| `edgion-gateway/src/lb/attempt.rs` | `lb-core` | source | pending | — |
| `edgion-gateway/src/lb/backend_selector/mod.rs` | `lb-backend-selector` | source | pending | — |
| `edgion-gateway/src/lb/backend_selector/selector.rs` | `lb-backend-selector` | source | pending | — |
| `edgion-gateway/src/lb/backend_selector/weighted_selector.rs` | `lb-backend-selector` | source | pending | — |
| `edgion-gateway/src/lb/compiled_selector.rs` | `lb-core` | source | pending | — |
| `edgion-gateway/src/lb/edgion_backend.rs` | `lb-core` | source | pending | — |
| `edgion-gateway/src/lb/ewma/metrics.rs` | `lb-ewma` | source | pending | — |
| `edgion-gateway/src/lb/ewma/mod.rs` | `lb-ewma` | source | pending | — |
| `edgion-gateway/src/lb/leastconn/cleaner.rs` | `lb-leastconn` | source | pending | — |
| `edgion-gateway/src/lb/leastconn/mod.rs` | `lb-leastconn` | source | pending | — |
| `edgion-gateway/src/lb/mod.rs` | `lb-core` | source | pending | — |
| `edgion-gateway/src/lb/runtime_state/mod.rs` | `lb-runtime-state` | source | pending | — |
| `edgion-gateway/src/lb/selection/consistent_hash.rs` | `lb-selection` | source | pending | — |
| `edgion-gateway/src/lb/selection/mod.rs` | `lb-selection` | source | pending | — |
| `edgion-gateway/src/lb/selection/p2c.rs` | `lb-selection` | source | pending | — |
| `edgion-gateway/src/lb/selection/round_robin.rs` | `lb-selection` | source | pending | — |
| `edgion-gateway/src/lib.rs` | `crate-entry` | source | pending | — |
| `edgion-gateway/src/link_sys/mod.rs` | `link-sys-root` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/elasticsearch/bulk.rs` | `link-sys-provider-elasticsearch` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/elasticsearch/client.rs` | `link-sys-provider-elasticsearch` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/elasticsearch/config_mapping.rs` | `link-sys-provider-elasticsearch` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/elasticsearch/log_sender.rs` | `link-sys-provider-elasticsearch` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/elasticsearch/mod.rs` | `link-sys-provider-elasticsearch` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/elasticsearch/ops.rs` | `link-sys-provider-elasticsearch` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/kafka/client.rs` | `link-sys-provider-kafka` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/kafka/log_sender.rs` | `link-sys-provider-kafka` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/kafka/mod.rs` | `link-sys-provider-kafka` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/local_file/data_sender_impl.rs` | `link-sys-provider-local-file` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/local_file/mod.rs` | `link-sys-provider-local-file` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/local_file/rotation.rs` | `link-sys-provider-local-file` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/mod.rs` | `link-sys-providers-root` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/otlp/log_sender.rs` | `link-sys-provider-otlp` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/otlp/mod.rs` | `link-sys-provider-otlp` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/redis/client.rs` | `link-sys-provider-redis` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/redis/config_mapping.rs` | `link-sys-provider-redis` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/redis/mod.rs` | `link-sys-provider-redis` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/redis/ops.rs` | `link-sys-provider-redis` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/webhook/client.rs` | `link-sys-provider-webhook` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/webhook/manager.rs` | `link-sys-provider-webhook` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/webhook/mod.rs` | `link-sys-provider-webhook` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/webhook/resolver.rs` | `link-sys-provider-webhook` | source | pending | — |
| `edgion-gateway/src/link_sys/providers/webhook/runtime.rs` | `link-sys-provider-webhook` | source | pending | — |
| `edgion-gateway/src/link_sys/readme` | `link-sys-root` | documentation | pending | — |
| `edgion-gateway/src/link_sys/runtime/conf_handler.rs` | `link-sys-runtime` | source | pending | — |
| `edgion-gateway/src/link_sys/runtime/data_sender.rs` | `link-sys-runtime` | source | pending | — |
| `edgion-gateway/src/link_sys/runtime/health_probe.rs` | `link-sys-runtime` | source | pending | — |
| `edgion-gateway/src/link_sys/runtime/mod.rs` | `link-sys-runtime` | source | pending | — |
| `edgion-gateway/src/link_sys/runtime/store.rs` | `link-sys-runtime` | source | pending | — |
| `edgion-gateway/src/link_sys/template/engine.rs` | `link-sys-template` | source | pending | — |
| `edgion-gateway/src/link_sys/template/mod.rs` | `link-sys-template` | source | pending | — |
| `edgion-gateway/src/observe/access_log/backfill.rs` | `observe-access-log` | source | pending | — |
| `edgion-gateway/src/observe/access_log/entry.rs` | `observe-access-log` | source | pending | — |
| `edgion-gateway/src/observe/access_log/log_ext.rs` | `observe-access-log` | source | pending | — |
| `edgion-gateway/src/observe/access_log/logger.rs` | `observe-access-log` | source | pending | — |
| `edgion-gateway/src/observe/access_log/mod.rs` | `observe-access-log` | source | pending | — |
| `edgion-gateway/src/observe/access_log_store.rs` | `observe-core` | source | pending | — |
| `edgion-gateway/src/observe/logs/buffer.rs` | `observe-protocol-logs` | source | optimized | OBS-LOG-001 / `11d3e79c` |
| `edgion-gateway/src/observe/logs/logger_factory.rs` | `observe-protocol-logs` | source | optimized | OBS-LOG-001 / `11d3e79c` |
| `edgion-gateway/src/observe/logs/mod.rs` | `observe-protocol-logs` | source | reviewed-no-change | OBS-LOG-001 / `11d3e79c` |
| `edgion-gateway/src/observe/logs/ssl_log.rs` | `observe-protocol-logs` | source | optimized | OBS-LOG-001 / `11d3e79c` |
| `edgion-gateway/src/observe/logs/tcp_log.rs` | `observe-protocol-logs` | source | reviewed-no-change | OBS-LOG-001 / `11d3e79c` |
| `edgion-gateway/src/observe/logs/tls_log.rs` | `observe-protocol-logs` | source | reviewed-no-change | OBS-LOG-001 / `11d3e79c` |
| `edgion-gateway/src/observe/logs/udp_log.rs` | `observe-protocol-logs` | source | optimized | OBS-LOG-001 / `11d3e79c` |
| `edgion-gateway/src/observe/metrics/api.rs` | `observe-metrics` | source | pending | — |
| `edgion-gateway/src/observe/metrics/mod.rs` | `observe-metrics` | source | pending | — |
| `edgion-gateway/src/observe/metrics/registry.rs` | `observe-metrics` | source | pending | — |
| `edgion-gateway/src/observe/metrics/route_handles.rs` | `observe-metrics` | source | pending | — |
| `edgion-gateway/src/observe/metrics/test_metrics.rs` | `observe-metrics` | source | pending | — |
| `edgion-gateway/src/observe/mod.rs` | `observe-core` | source | pending | — |
| `edgion-gateway/src/observe/request_diagnostics.rs` | `observe-core` | source | pending | — |
| `edgion-gateway/src/plugins/common/mod.rs` | `plugins-common` | source | pending | — |
| `edgion-gateway/src/plugins/common/redis_rate_limiter/mod.rs` | `plugins-common` | source | pending | — |
| `edgion-gateway/src/plugins/common/redis_rate_limiter/scripts.rs` | `plugins-common` | source | pending | — |
| `edgion-gateway/src/plugins/http/ai_guard/evaluator.rs` | `plugins-http-ai-guard` | source | pending | — |
| `edgion-gateway/src/plugins/http/ai_guard/mod.rs` | `plugins-http-ai-guard` | source | pending | — |
| `edgion-gateway/src/plugins/http/ai_guard/plugin.rs` | `plugins-http-ai-guard` | source | pending | — |
| `edgion-gateway/src/plugins/http/ai_guard/rules.rs` | `plugins-http-ai-guard` | source | pending | — |
| `edgion-gateway/src/plugins/http/ai_guard/spans.rs` | `plugins-http-ai-guard` | source | pending | — |
| `edgion-gateway/src/plugins/http/ai_proxy/mod.rs` | `plugins-http-ai-proxy` | source | pending | — |
| `edgion-gateway/src/plugins/http/ai_proxy/plugin.rs` | `plugins-http-ai-proxy` | source | pending | — |
| `edgion-gateway/src/plugins/http/all_endpoint_status/mod.rs` | `plugins-http-all-endpoint-status` | source | pending | — |
| `edgion-gateway/src/plugins/http/all_endpoint_status/plugin.rs` | `plugins-http-all-endpoint-status` | source | pending | — |
| `edgion-gateway/src/plugins/http/bandwidth_limit/mod.rs` | `plugins-http-bandwidth-limit` | source | pending | — |
| `edgion-gateway/src/plugins/http/bandwidth_limit/plugin.rs` | `plugins-http-bandwidth-limit` | source | pending | — |
| `edgion-gateway/src/plugins/http/basic_auth/htpasswd.rs` | `plugins-http-basic-auth` | source | pending | — |
| `edgion-gateway/src/plugins/http/basic_auth/mod.rs` | `plugins-http-basic-auth` | source | pending | — |
| `edgion-gateway/src/plugins/http/basic_auth/plugin.rs` | `plugins-http-basic-auth` | source | pending | — |
| `edgion-gateway/src/plugins/http/canary/mod.rs` | `plugins-http-canary` | source | pending | — |
| `edgion-gateway/src/plugins/http/canary/plugin.rs` | `plugins-http-canary` | source | pending | — |
| `edgion-gateway/src/plugins/http/common/auth_common.rs` | `plugins-http-common` | source | pending | — |
| `edgion-gateway/src/plugins/http/common/capped_writer.rs` | `plugins-http-common` | source | pending | — |
| `edgion-gateway/src/plugins/http/common/http_client.rs` | `plugins-http-common` | source | pending | — |
| `edgion-gateway/src/plugins/http/common/http_util.rs` | `plugins-http-common` | source | pending | — |
| `edgion-gateway/src/plugins/http/common/json_request.rs` | `plugins-http-common` | source | pending | — |
| `edgion-gateway/src/plugins/http/common/mod.rs` | `plugins-http-common` | source | pending | — |
| `edgion-gateway/src/plugins/http/common/outbound/call.rs` | `plugins-http-common` | source | pending | — |
| `edgion-gateway/src/plugins/http/common/outbound/client.rs` | `plugins-http-common` | source | pending | — |
| `edgion-gateway/src/plugins/http/common/outbound/egress.rs` | `plugins-http-common` | source | pending | — |
| `edgion-gateway/src/plugins/http/common/outbound/mod.rs` | `plugins-http-common` | source | pending | — |
| `edgion-gateway/src/plugins/http/common/outbound/pinned_resolver.rs` | `plugins-http-common` | source | pending | — |
| `edgion-gateway/src/plugins/http/common/outbound/request.rs` | `plugins-http-common` | source | pending | — |
| `edgion-gateway/src/plugins/http/common/outbound/target.rs` | `plugins-http-common` | source | pending | — |
| `edgion-gateway/src/plugins/http/conf_handler_impl.rs` | `plugins-http-framework` | source | pending | — |
| `edgion-gateway/src/plugins/http/cors/mod.rs` | `plugins-http-cors` | source | pending | — |
| `edgion-gateway/src/plugins/http/cors/plugin.rs` | `plugins-http-cors` | source | pending | — |
| `edgion-gateway/src/plugins/http/credential_injector/mod.rs` | `plugins-http-credential-injector` | source | pending | — |
| `edgion-gateway/src/plugins/http/credential_injector/plugin.rs` | `plugins-http-credential-injector` | source | pending | — |
| `edgion-gateway/src/plugins/http/csrf/mod.rs` | `plugins-http-csrf` | source | pending | — |
| `edgion-gateway/src/plugins/http/csrf/plugin.rs` | `plugins-http-csrf` | source | pending | — |
| `edgion-gateway/src/plugins/http/csrf/token.rs` | `plugins-http-csrf` | source | pending | — |
| `edgion-gateway/src/plugins/http/ctx_set/mod.rs` | `plugins-http-ctx-set` | source | pending | — |
| `edgion-gateway/src/plugins/http/ctx_set/plugin.rs` | `plugins-http-ctx-set` | source | pending | — |
| `edgion-gateway/src/plugins/http/direct_endpoint/mod.rs` | `plugins-http-direct-endpoint` | source | pending | — |
| `edgion-gateway/src/plugins/http/direct_endpoint/plugin.rs` | `plugins-http-direct-endpoint` | source | pending | — |
| `edgion-gateway/src/plugins/http/dsl/config.rs` | `plugins-http-dsl` | source | pending | — |
| `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs` | `plugins-http-dsl` | source | pending | — |
| `edgion-gateway/src/plugins/http/dsl/lang/mod.rs` | `plugins-http-dsl` | source | pending | — |
| `edgion-gateway/src/plugins/http/dsl/lang/vm.rs` | `plugins-http-dsl` | source | pending | — |
| `edgion-gateway/src/plugins/http/dsl/mod.rs` | `plugins-http-dsl` | source | pending | — |
| `edgion-gateway/src/plugins/http/dsl/plugin.rs` | `plugins-http-dsl` | source | pending | — |
| `edgion-gateway/src/plugins/http/dsl/runtime.rs` | `plugins-http-dsl` | source | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/api_errors.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/body_value.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/bytecode_deserialization.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/compile_errors.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/control_flow.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/ctx_builtins.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/decision_log_tests.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/error_policy.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/grpc_error_tests.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/http_call_tests.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/mod.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/p3_utility_tests.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/property_bytecode.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/range_and_string_ops.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/redis_builtins.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/req_metadata_tests.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/request_builtins.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/request_mutation.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/resp_p2_tests.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/resp_set_status_tests.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/response_builtins.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/runtime_errors.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/sandbox.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/terminate_tests.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/utility_functions.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dsl/tests/vm_defensive_tests.rs` | `plugins-http-dsl` | test | pending | — |
| `edgion-gateway/src/plugins/http/dynamic_external_upstream/mod.rs` | `plugins-http-dynamic-external-upstream` | source | pending | — |
| `edgion-gateway/src/plugins/http/dynamic_external_upstream/plugin.rs` | `plugins-http-dynamic-external-upstream` | source | pending | — |
| `edgion-gateway/src/plugins/http/dynamic_internal_upstream/mod.rs` | `plugins-http-dynamic-internal-upstream` | source | pending | — |
| `edgion-gateway/src/plugins/http/dynamic_internal_upstream/plugin.rs` | `plugins-http-dynamic-internal-upstream` | source | pending | — |
| `edgion-gateway/src/plugins/http/ext_proc/attached.rs` | `plugins-http-ext-proc` | source | pending | — |
| `edgion-gateway/src/plugins/http/ext_proc/client.rs` | `plugins-http-ext-proc` | source | pending | — |
| `edgion-gateway/src/plugins/http/ext_proc/mod.rs` | `plugins-http-ext-proc` | source | pending | — |
| `edgion-gateway/src/plugins/http/ext_proc/mutation.rs` | `plugins-http-ext-proc` | source | pending | — |
| `edgion-gateway/src/plugins/http/ext_proc/plugin.rs` | `plugins-http-ext-proc` | source | pending | — |
| `edgion-gateway/src/plugins/http/ext_proc/response_streamed.rs` | `plugins-http-ext-proc` | source | pending | — |
| `edgion-gateway/src/plugins/http/ext_proc/response_trailer.rs` | `plugins-http-ext-proc` | source | pending | — |
| `edgion-gateway/src/plugins/http/ext_proc/streamed.rs` | `plugins-http-ext-proc` | source | pending | — |
| `edgion-gateway/src/plugins/http/fault_injection/mod.rs` | `plugins-http-fault-injection` | source | pending | — |
| `edgion-gateway/src/plugins/http/fault_injection/plugin.rs` | `plugins-http-fault-injection` | source | pending | — |
| `edgion-gateway/src/plugins/http/form_json_transform/mod.rs` | `plugins-http-form-json-transform` | source | pending | — |
| `edgion-gateway/src/plugins/http/form_json_transform/plugin.rs` | `plugins-http-form-json-transform` | source | pending | — |
| `edgion-gateway/src/plugins/http/forward_auth/mod.rs` | `plugins-http-forward-auth` | source | pending | — |
| `edgion-gateway/src/plugins/http/forward_auth/plugin.rs` | `plugins-http-forward-auth` | source | pending | — |
| `edgion-gateway/src/plugins/http/geoip_location/mod.rs` | `plugins-http-geoip-location` | source | pending | — |
| `edgion-gateway/src/plugins/http/geoip_location/plugin.rs` | `plugins-http-geoip-location` | source | pending | — |
| `edgion-gateway/src/plugins/http/guardrail/attached.rs` | `plugins-http-guardrail` | source | pending | — |
| `edgion-gateway/src/plugins/http/guardrail/mod.rs` | `plugins-http-guardrail` | source | pending | — |
| `edgion-gateway/src/plugins/http/guardrail/plugin.rs` | `plugins-http-guardrail` | source | pending | — |
| `edgion-gateway/src/plugins/http/guardrail/protocol.rs` | `plugins-http-guardrail` | source | pending | — |
| `edgion-gateway/src/plugins/http/guardrail/response.rs` | `plugins-http-guardrail` | source | pending | — |
| `edgion-gateway/src/plugins/http/header_cert_auth/mod.rs` | `plugins-http-header-cert-auth` | source | pending | — |
| `edgion-gateway/src/plugins/http/header_cert_auth/plugin.rs` | `plugins-http-header-cert-auth` | source | pending | — |
| `edgion-gateway/src/plugins/http/hmac_auth/mod.rs` | `plugins-http-hmac-auth` | source | pending | — |
| `edgion-gateway/src/plugins/http/hmac_auth/plugin.rs` | `plugins-http-hmac-auth` | source | pending | — |
| `edgion-gateway/src/plugins/http/ip_restriction/mod.rs` | `plugins-http-ip-restriction` | source | pending | — |
| `edgion-gateway/src/plugins/http/ip_restriction/plugin.rs` | `plugins-http-ip-restriction` | source | pending | — |
| `edgion-gateway/src/plugins/http/json_schema_validation/mod.rs` | `plugins-http-json-schema-validation` | source | pending | — |
| `edgion-gateway/src/plugins/http/json_schema_validation/plugin.rs` | `plugins-http-json-schema-validation` | source | pending | — |
| `edgion-gateway/src/plugins/http/jwe_decrypt/mod.rs` | `plugins-http-jwe-decrypt` | source | pending | — |
| `edgion-gateway/src/plugins/http/jwe_decrypt/plugin.rs` | `plugins-http-jwe-decrypt` | source | pending | — |
| `edgion-gateway/src/plugins/http/jwt_auth/credentials.rs` | `plugins-http-jwt-auth` | source | pending | — |
| `edgion-gateway/src/plugins/http/jwt_auth/mod.rs` | `plugins-http-jwt-auth` | source | pending | — |
| `edgion-gateway/src/plugins/http/jwt_auth/plugin.rs` | `plugins-http-jwt-auth` | source | pending | — |
| `edgion-gateway/src/plugins/http/jwt_auth/request_stage.rs` | `plugins-http-jwt-auth` | source | pending | — |
| `edgion-gateway/src/plugins/http/jwt_auth/tests/algorithm.rs` | `plugins-http-jwt-auth` | test | pending | — |
| `edgion-gateway/src/plugins/http/jwt_auth/tests/anonymous.rs` | `plugins-http-jwt-auth` | test | pending | — |
| `edgion-gateway/src/plugins/http/jwt_auth/tests/claim_propagation.rs` | `plugins-http-jwt-auth` | test | pending | — |
| `edgion-gateway/src/plugins/http/jwt_auth/tests/claim_validation.rs` | `plugins-http-jwt-auth` | test | pending | — |
| `edgion-gateway/src/plugins/http/jwt_auth/tests/common.rs` | `plugins-http-jwt-auth` | test | pending | — |
| `edgion-gateway/src/plugins/http/jwt_auth/tests/error_classification.rs` | `plugins-http-jwt-auth` | test | pending | — |
| `edgion-gateway/src/plugins/http/jwt_auth/tests/failure_response.rs` | `plugins-http-jwt-auth` | test | pending | — |
| `edgion-gateway/src/plugins/http/jwt_auth/tests/mod.rs` | `plugins-http-jwt-auth` | test | pending | — |
| `edgion-gateway/src/plugins/http/jwt_auth/tests/secret_decoding.rs` | `plugins-http-jwt-auth` | test | pending | — |
| `edgion-gateway/src/plugins/http/jwt_auth/tests/token_source.rs` | `plugins-http-jwt-auth` | test | pending | — |
| `edgion-gateway/src/plugins/http/jwt_auth/verify.rs` | `plugins-http-jwt-auth` | source | pending | — |
| `edgion-gateway/src/plugins/http/key_auth/mod.rs` | `plugins-http-key-auth` | source | pending | — |
| `edgion-gateway/src/plugins/http/key_auth/plugin.rs` | `plugins-http-key-auth` | source | pending | — |
| `edgion-gateway/src/plugins/http/ldap_auth/mod.rs` | `plugins-http-ldap-auth` | source | pending | — |
| `edgion-gateway/src/plugins/http/ldap_auth/plugin.rs` | `plugins-http-ldap-auth` | source | pending | — |
| `edgion-gateway/src/plugins/http/mock/mod.rs` | `plugins-http-mock` | source | pending | — |
| `edgion-gateway/src/plugins/http/mock/plugin.rs` | `plugins-http-mock` | source | pending | — |
| `edgion-gateway/src/plugins/http/mod.rs` | `plugins-http-framework` | source | pending | — |
| `edgion-gateway/src/plugins/http/openid_connect/callback.rs` | `plugins-http-openid-connect` | source | pending | — |
| `edgion-gateway/src/plugins/http/openid_connect/config_validation.rs` | `plugins-http-openid-connect` | source | pending | — |
| `edgion-gateway/src/plugins/http/openid_connect/discovery_jwks.rs` | `plugins-http-openid-connect` | source | pending | — |
| `edgion-gateway/src/plugins/http/openid_connect/error.rs` | `plugins-http-openid-connect` | source | pending | — |
| `edgion-gateway/src/plugins/http/openid_connect/jwks_provider.rs` | `plugins-http-openid-connect` | source | pending | — |
| `edgion-gateway/src/plugins/http/openid_connect/jwt_common.rs` | `plugins-http-openid-connect` | source | pending | — |
| `edgion-gateway/src/plugins/http/openid_connect/logout.rs` | `plugins-http-openid-connect` | source | pending | — |
| `edgion-gateway/src/plugins/http/openid_connect/mod.rs` | `plugins-http-openid-connect` | source | pending | — |
| `edgion-gateway/src/plugins/http/openid_connect/plugin.rs` | `plugins-http-openid-connect` | source | pending | — |
| `edgion-gateway/src/plugins/http/openid_connect/refresh_flight_registry.rs` | `plugins-http-openid-connect` | source | pending | — |
| `edgion-gateway/src/plugins/http/openid_connect/request_filter_impl.rs` | `plugins-http-openid-connect` | source | pending | — |
| `edgion-gateway/src/plugins/http/openid_connect/session_cookie.rs` | `plugins-http-openid-connect` | source | pending | — |
| `edgion-gateway/src/plugins/http/openid_connect/tests.rs` | `plugins-http-openid-connect` | test | pending | — |
| `edgion-gateway/src/plugins/http/openid_connect/token_exchange.rs` | `plugins-http-openid-connect` | source | optimized | CACHE-001 / `40f7eb74` (reviewed early with cache consumer) |
| `edgion-gateway/src/plugins/http/openid_connect/token_verify.rs` | `plugins-http-openid-connect` | source | pending | — |
| `edgion-gateway/src/plugins/http/openid_connect/userinfo.rs` | `plugins-http-openid-connect` | source | pending | — |
| `edgion-gateway/src/plugins/http/plugin_store.rs` | `plugins-http-framework` | source | pending | — |
| `edgion-gateway/src/plugins/http/proxy_rewrite/mod.rs` | `plugins-http-proxy-rewrite` | source | pending | — |
| `edgion-gateway/src/plugins/http/proxy_rewrite/plugin.rs` | `plugins-http-proxy-rewrite` | source | pending | — |
| `edgion-gateway/src/plugins/http/rate_limit/mod.rs` | `plugins-http-rate-limit` | source | pending | — |
| `edgion-gateway/src/plugins/http/rate_limit/plugin.rs` | `plugins-http-rate-limit` | source | pending | — |
| `edgion-gateway/src/plugins/http/rate_limit_redis/mod.rs` | `plugins-http-rate-limit-redis` | source | pending | — |
| `edgion-gateway/src/plugins/http/rate_limit_redis/plugin.rs` | `plugins-http-rate-limit-redis` | source | pending | — |
| `edgion-gateway/src/plugins/http/real_ip/mod.rs` | `plugins-http-real-ip` | source | pending | — |
| `edgion-gateway/src/plugins/http/real_ip/plugin.rs` | `plugins-http-real-ip` | source | pending | — |
| `edgion-gateway/src/plugins/http/region_route/log_consts.rs` | `plugins-http-region-route` | source | pending | — |
| `edgion-gateway/src/plugins/http/region_route/mod.rs` | `plugins-http-region-route` | source | pending | — |
| `edgion-gateway/src/plugins/http/region_route/plugin.rs` | `plugins-http-region-route` | source | pending | — |
| `edgion-gateway/src/plugins/http/request_access_policy/mod.rs` | `plugins-http-request-access-policy` | source | pending | — |
| `edgion-gateway/src/plugins/http/request_access_policy/plugin.rs` | `plugins-http-request-access-policy` | source | pending | — |
| `edgion-gateway/src/plugins/http/request_body_buffer/mod.rs` | `plugins-http-request-body-buffer` | source | pending | — |
| `edgion-gateway/src/plugins/http/request_body_buffer/plugin.rs` | `plugins-http-request-body-buffer` | source | pending | — |
| `edgion-gateway/src/plugins/http/request_mirror/mirror_log.rs` | `plugins-http-request-mirror` | source | pending | — |
| `edgion-gateway/src/plugins/http/request_mirror/mirror_state.rs` | `plugins-http-request-mirror` | source | pending | — |
| `edgion-gateway/src/plugins/http/request_mirror/mod.rs` | `plugins-http-request-mirror` | source | pending | — |
| `edgion-gateway/src/plugins/http/request_mirror/plugin.rs` | `plugins-http-request-mirror` | source | pending | — |
| `edgion-gateway/src/plugins/http/request_restriction/mod.rs` | `plugins-http-request-restriction` | source | pending | — |
| `edgion-gateway/src/plugins/http/request_restriction/plugin.rs` | `plugins-http-request-restriction` | source | pending | — |
| `edgion-gateway/src/plugins/http/response_rewrite/mod.rs` | `plugins-http-response-rewrite` | source | pending | — |
| `edgion-gateway/src/plugins/http/response_rewrite/plugin.rs` | `plugins-http-response-rewrite` | source | pending | — |
| `edgion-gateway/src/plugins/http/trace_context/mod.rs` | `plugins-http-trace-context` | source | pending | — |
| `edgion-gateway/src/plugins/http/trace_context/plugin.rs` | `plugins-http-trace-context` | source | pending | — |
| `edgion-gateway/src/plugins/http/trace_context/w3c.rs` | `plugins-http-trace-context` | source | pending | — |
| `edgion-gateway/src/plugins/http/waf/mod.rs` | `plugins-http-waf` | source | pending | — |
| `edgion-gateway/src/plugins/http/wasm/callout.rs` | `plugins-http-wasm` | source | pending | — |
| `edgion-gateway/src/plugins/http/wasm/host_abi.rs` | `plugins-http-wasm` | source | pending | — |
| `edgion-gateway/src/plugins/http/wasm/mod.rs` | `plugins-http-wasm` | source | pending | — |
| `edgion-gateway/src/plugins/http/wasm/oci/auth.rs` | `plugins-http-wasm` | source | pending | — |
| `edgion-gateway/src/plugins/http/wasm/oci/cache.rs` | `plugins-http-wasm` | source | pending | — |
| `edgion-gateway/src/plugins/http/wasm/oci/manifest.rs` | `plugins-http-wasm` | source | pending | — |
| `edgion-gateway/src/plugins/http/wasm/oci/mod.rs` | `plugins-http-wasm` | source | pending | — |
| `edgion-gateway/src/plugins/http/wasm/plugin.rs` | `plugins-http-wasm` | source | pending | — |
| `edgion-gateway/src/plugins/http/wasm/shared_data.rs` | `plugins-http-wasm` | source | pending | — |
| `edgion-gateway/src/plugins/http/wasm/vm_manager.rs` | `plugins-http-wasm` | source | pending | — |
| `edgion-gateway/src/plugins/http/webhook_key_get/mod.rs` | `plugins-http-webhook-key-get` | source | pending | — |
| `edgion-gateway/src/plugins/http/webhook_key_get/plugin.rs` | `plugins-http-webhook-key-get` | source | pending | — |
| `edgion-gateway/src/plugins/mod.rs` | `plugins-root` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/body_gate.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/conditional_filter.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/conditions/evaluator.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/conditions/extract.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/conditions/mod.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/dye_plan.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/edgion_config_data/conf_handler.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/edgion_config_data/mod.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/cors_filter.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/debug_access_log.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/mod.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/request_header_modifier.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/request_redirect.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/response_header_modifier.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/url_rewrite.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/log.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/mod.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/plugin_runtime.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/policy_filter.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/reject_filter.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/session_adapter.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/session_adapter/storage_io_tests.rs` | `plugins-runtime` | test | pending | — |
| `edgion-gateway/src/plugins/runtime/traits/mod.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/traits/request_body_stream.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/traits/request_filter.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/traits/session.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/traits/streaming_response_processor.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/traits/test_helpers.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/traits/upstream_response.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/traits/upstream_response_body_filter.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/traits/upstream_response_filter.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/runtime/traits/upstream_response_trailer.rs` | `plugins-runtime` | source | pending | — |
| `edgion-gateway/src/plugins/stream/connection_filter_bridge.rs` | `plugins-stream` | source | pending | — |
| `edgion-gateway/src/plugins/stream/connection_rate_limit/local_gate.rs` | `plugins-stream` | source | pending | — |
| `edgion-gateway/src/plugins/stream/connection_rate_limit/mod.rs` | `plugins-stream` | source | pending | — |
| `edgion-gateway/src/plugins/stream/connection_rate_limit/plugin.rs` | `plugins-stream` | source | pending | — |
| `edgion-gateway/src/plugins/stream/geoip_location/mod.rs` | `plugins-stream` | source | pending | — |
| `edgion-gateway/src/plugins/stream/geoip_location/plugin.rs` | `plugins-stream` | source | pending | — |
| `edgion-gateway/src/plugins/stream/global_connection_ip_restriction/mod.rs` | `plugins-stream` | source | pending | — |
| `edgion-gateway/src/plugins/stream/global_connection_ip_restriction/plugin.rs` | `plugins-stream` | source | pending | — |
| `edgion-gateway/src/plugins/stream/ip_restriction/mod.rs` | `plugins-stream` | source | pending | — |
| `edgion-gateway/src/plugins/stream/ip_restriction/stream_ip_restriction.rs` | `plugins-stream` | source | pending | — |
| `edgion-gateway/src/plugins/stream/mod.rs` | `plugins-stream` | source | pending | — |
| `edgion-gateway/src/plugins/stream/stream_plugin_runtime.rs` | `plugins-stream` | source | pending | — |
| `edgion-gateway/src/plugins/stream/stream_plugin_store.rs` | `plugins-stream` | source | pending | — |
| `edgion-gateway/src/plugins/stream/stream_plugin_trait.rs` | `plugins-stream` | source | pending | — |
| `edgion-gateway/src/plugins/stream/tls_route/ip_restriction.rs` | `plugins-stream` | source | pending | — |
| `edgion-gateway/src/plugins/stream/tls_route/mod.rs` | `plugins-stream` | source | pending | — |
| `edgion-gateway/src/plugins/stream/tls_route/tls_route_plugin_runtime.rs` | `plugins-stream` | source | pending | — |
| `edgion-gateway/src/plugins/stream/tls_route/tls_route_plugin_trait.rs` | `plugins-stream` | source | pending | — |
| `edgion-gateway/src/request_body/body_scanners.rs` | `request-body` | source | pending | — |
| `edgion-gateway/src/request_body/config.rs` | `request-body` | source | pending | — |
| `edgion-gateway/src/request_body/mod.rs` | `request-body` | source | pending | — |
| `edgion-gateway/src/request_body/observer.rs` | `request-body` | source | pending | — |
| `edgion-gateway/src/request_body/operation.rs` | `request-body` | source | pending | — |
| `edgion-gateway/src/request_body/state.rs` | `request-body` | source | pending | — |
| `edgion-gateway/src/request_body/state/enabled.rs` | `request-body` | source | pending | — |
| `edgion-gateway/src/request_body/storage.rs` | `request-body` | source | pending | — |
| `edgion-gateway/src/request_body/storage_io_tests.rs` | `request-body` | test | pending | — |
| `edgion-gateway/src/routes/common.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/grpc/conf_handler_impl.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/grpc/grpc_timeout.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/grpc/integration.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/grpc/match_engine.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/grpc/match_unit.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/grpc/mod.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/grpc/routes_mgr.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/conf_handler_impl.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/match_engine/mod.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/match_engine/radix_path.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/match_engine/radix_route_match.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/match_engine/regex_routes_engine.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/match_unit.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/mod.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/content_type.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/global_plugins_resolve.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/mod.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/parse_timeout.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/pg_connected_to_upstream.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/pg_early_request_filter.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/pg_error_while_proxy.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/pg_fail_to_connect.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/pg_fail_to_proxy.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/pg_init_downstream_modules.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/pg_logging.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/pg_new_ctx.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/pg_request_body_filter.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/pg_request_body_filter_action.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/pg_response_filter.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/pg_upstream_request_filter.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_body_filter.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_filter.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_trailer_filter.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/proxy_http/preflight_handler.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/redirect_http.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/routes_mgr.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/session_persistence.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/http/tests.rs` | `routes-shared` | test | pending | — |
| `edgion-gateway/src/routes/l4_duplex.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/l4_partial_update.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/l4_rebuild.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/mod.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/tcp/conf_handler_impl.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/tcp/edgion_tcp.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/tcp/mod.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/tcp/route_unit.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/tcp/routes_mgr.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/tcp/tcp_route_table.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/tls/conf_handler_impl.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/tls/gateway_tls_routes.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/tls/mod.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/tls/passthrough_proxy.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/tls/proxy.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/tls/route_unit.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/tls/routes_mgr.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/tls/sni_parser.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/udp/conf_handler_impl.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/udp/edgion_udp.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/udp/mod.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/udp/route_unit.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/udp/routes_mgr.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/routes/udp/udp_route_table.rs` | `routes-shared` | source | pending | — |
| `edgion-gateway/src/runtime/gateway_info.rs` | `runtime` | source | pending | — |
| `edgion-gateway/src/runtime/handler.rs` | `runtime` | source | pending | — |
| `edgion-gateway/src/runtime/matching/mod.rs` | `runtime` | source | pending | — |
| `edgion-gateway/src/runtime/matching/route.rs` | `runtime` | source | pending | — |
| `edgion-gateway/src/runtime/matching/tests/tls_test.rs` | `runtime` | test | pending | — |
| `edgion-gateway/src/runtime/matching/tls.rs` | `runtime` | source | pending | — |
| `edgion-gateway/src/runtime/mod.rs` | `runtime` | source | pending | — |
| `edgion-gateway/src/runtime/server/base.rs` | `runtime` | source | pending | — |
| `edgion-gateway/src/runtime/server/error_response.rs` | `runtime` | source | pending | — |
| `edgion-gateway/src/runtime/server/listener_builder.rs` | `runtime` | source | pending | — |
| `edgion-gateway/src/runtime/server/mod.rs` | `runtime` | source | pending | — |
| `edgion-gateway/src/runtime/server/server_header.rs` | `runtime` | source | pending | — |
| `edgion-gateway/src/runtime/store/gateway.rs` | `runtime` | source | pending | — |
| `edgion-gateway/src/runtime/store/listener_gateway_info.rs` | `runtime` | source | pending | — |
| `edgion-gateway/src/runtime/store/mod.rs` | `runtime` | source | pending | — |
| `edgion-gateway/src/services/acme/challenge_store.rs` | `services` | source | pending | — |
| `edgion-gateway/src/services/acme/conf_handler_impl.rs` | `services` | source | pending | — |
| `edgion-gateway/src/services/acme/mod.rs` | `services` | source | pending | — |
| `edgion-gateway/src/services/mod.rs` | `services` | source | pending | — |
| `edgion-gateway/src/sse/chain.rs` | `sse` | source | pending | — |
| `edgion-gateway/src/sse/mod.rs` | `sse` | source | pending | — |
| `edgion-gateway/src/sse/window.rs` | `sse` | source | pending | — |
| `edgion-gateway/src/tls/boringssl/mod.rs` | `tls-boringssl` | source | pending | — |
| `edgion-gateway/src/tls/boringssl/mtls_verify_callback.rs` | `tls-boringssl` | source | pending | — |
| `edgion-gateway/src/tls/fips_host.rs` | `tls-core` | source | pending | — |
| `edgion-gateway/src/tls/fips_profile.rs` | `tls-core` | source | pending | — |
| `edgion-gateway/src/tls/mod.rs` | `tls-core` | source | pending | — |
| `edgion-gateway/src/tls/runtime/backend/backend_api.rs` | `tls-runtime` | source | pending | — |
| `edgion-gateway/src/tls/runtime/backend/cert_extractor.rs` | `tls-runtime` | source | pending | — |
| `edgion-gateway/src/tls/runtime/backend/mod.rs` | `tls-runtime` | source | pending | — |
| `edgion-gateway/src/tls/runtime/gateway/mod.rs` | `tls-runtime` | source | pending | — |
| `edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs` | `tls-runtime` | source | pending | — |
| `edgion-gateway/src/tls/runtime/mod.rs` | `tls-runtime` | source | pending | — |
| `edgion-gateway/src/tls/store/cert_matcher.rs` | `tls-store` | source | pending | — |
| `edgion-gateway/src/tls/store/conf_handler.rs` | `tls-store` | source | pending | — |
| `edgion-gateway/src/tls/store/mod.rs` | `tls-store` | source | pending | — |
| `edgion-gateway/src/tls/store/tls_store.rs` | `tls-store` | source | pending | — |
| `edgion-gateway/src/tls/validation/backend_san.rs` | `tls-validation` | source | pending | — |
| `edgion-gateway/src/tls/validation/cert.rs` | `tls-validation` | source | pending | — |
| `edgion-gateway/src/tls/validation/mod.rs` | `tls-validation` | source | pending | — |
| `edgion-gateway/src/tls/validation/mtls.rs` | `tls-validation` | source | pending | — |
| `edgion-gateway/src/tls/validation/outbound_san_verifier.rs` | `tls-validation` | source | pending | — |
| `edgion-gateway/src/util/client_ip.rs` | `util` | source | optimized | UTIL-001 / `a909f16f` |
| `edgion-gateway/src/util/mod.rs` | `util` | source | reviewed-no-change | UTIL-001 / `a909f16f` |
| `edgion-gateway/src/util/net.rs` | `util` | source | optimized | UTIL-001 / `a909f16f` |
| `edgion-gateway/src/util/proxy_protocol.rs` | `util` | source | optimized | UTIL-001 / `a909f16f` |
| `edgion-gateway/src/util/real_ip_extractor.rs` | `util` | source | optimized | UTIL-001 / `a909f16f` |
| `edgion-gateway/src/util/redacted.rs` | `util` | source | reviewed-no-change | UTIL-001 / `a909f16f` |
| `edgion-gateway/tests/geoip_fixture_readback.rs` | `external-tests` | test | pending | — |
