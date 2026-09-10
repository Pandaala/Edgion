# Gateway File Inventory

## Frozen baseline

- Baseline commit: `d484e580c8274ce58426c9fe6729e444f505bdaf`
- Baseline Gateway tracked files: 530
- Baseline integration-test tree: `ae68dedd863020ddd6d035d7404570716429ecd1`
- Existing integration-test files may not be modified, renamed, or deleted. The user's additional constraint defers needed new integration coverage to issues; externally committed updates are reconciled individually, not edited by the campaign.
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
- `analysis-in-progress`: a read-only candidate is being investigated outside the serial mutation lane.
- `analysis-ready`: a read-only candidate report is complete and awaits current-HEAD promotion.
- `in-progress`: owned by the active frozen batch.
- `examined-no-change`: complete audit found no worthwhile safe change.
- `changed`: changed, reviewed, verified, and committed.
- `deferred`: a real issue requires a wider independent task.
- `excluded-with-reason`: generated, fixture, or explicitly out-of-scope content with stable evidence.
- `removed`: absent from the current tree with commit evidence.
- `renamed`: replaced by a destination row with commit evidence.

## Batch coverage

| Batch | Expected | Changed | No change | Other closed | Status |
|-------|---------:|----------:|----------:|-------------:|--------|
| `ai-anthropic` | 3 | 0 | 0 | 0 | pending |
| `ai-core` | 16 | 0 | 0 | 0 | pending |
| `ai-quota` | 2 | 0 | 0 | 0 | pending |
| `api` | 2 | 0 | 0 | 0 | pending |
| `backends-circuit-breaker` | 2 | 1 | 1 | 0 | complete |
| `backends-core` | 8 | 5 | 2 | 1 | complete |
| `backends-discovery-endpoints` | 4 | 1 | 1 | 2 | complete |
| `backends-discovery-services` | 3 | 0 | 3 | 0 | complete |
| `backends-discovery-shared` | 3 | 1 | 1 | 1 | complete |
| `backends-health` | 6 | 2 | 3 | 1 | complete |
| `backends-outlier` | 4 | 2 | 2 | 0 | complete |
| `backends-degrade-threshold` | 1 | 1 | 0 | 0 | complete |
| `backends-policy-root` | 1 | 0 | 1 | 0 | complete |
| `backends-policy-tls` | 3 | 1 | 2 | 0 | complete |
| `backends-policy-traffic` | 5 | 0 | 5 | 0 | complete |
| `backends-port-resolver` | 1 | 0 | 1 | 0 | complete |
| `cache` | 4 | 2 | 2 | 0 | complete |
| `cli` | 3 | 0 | 0 | 0 | pending |
| `conf-sync-cache-client` | 4 | 1 | 3 | 0 | complete |
| `conf-sync-conf-client` | 5 | 1 | 4 | 0 | complete |
| `conf-sync-root` | 1 | 0 | 1 | 0 | complete |
| `config-edgion-gateway` | 2 | 1 | 1 | 0 | complete |
| `config-gateway-class` | 2 | 0 | 2 | 0 | complete |
| `config-root` | 1 | 0 | 1 | 0 | complete |
| `crate-entry` | 2 | 0 | 2 | 0 | complete |
| `crate-root` | 2 | 0 | 0 | 0 | pending |
| `external-tests` | 3 | 0 | 0 | 0 | pending |
| `geoip` | 5 | 2 | 2 | 1 | complete |
| `http-context` | 1 | 0 | 0 | 0 | pending |
| `lb-backend-selector` | 3 | 0 | 0 | 0 | pending |
| `lb-core` | 4 | 0 | 0 | 0 | pending |
| `lb-ewma` | 2 | 0 | 0 | 0 | pending |
| `lb-leastconn` | 2 | 0 | 0 | 0 | pending |
| `lb-runtime-state` | 1 | 0 | 0 | 0 | pending |
| `lb-selection` | 4 | 0 | 0 | 0 | pending |
| `link-sys-provider-elasticsearch` | 6 | 1 | 5 | 0 | complete |
| `link-sys-provider-kafka` | 3 | 1 | 2 | 0 | complete |
| `link-sys-provider-local-file` | 3 | 2 | 1 | 0 | complete |
| `link-sys-provider-otlp` | 2 | 1 | 1 | 0 | complete |
| `link-sys-provider-redis` | 4 | 1 | 3 | 0 | complete |
| `link-sys-provider-webhook` | 5 | 3 | 1 | 1 | complete |
| `link-sys-providers-root` | 1 | 0 | 1 | 0 | complete |
| `link-sys-root` | 2 | 2 | 0 | 0 | complete |
| `link-sys-runtime` | 5 | 3 | 2 | 0 | complete |
| `link-sys-template` | 2 | 0 | 0 | 2 | complete |
| `observe-access-log` | 5 | 3 | 2 | 0 | complete |
| `observe-core` | 3 | 1 | 2 | 0 | complete |
| `observe-metrics` | 5 | 3 | 2 | 0 | complete |
| `observe-protocol-logs` | 7 | 4 | 3 | 0 | complete |
| `plugins-common` | 3 | 2 | 1 | 0 | complete |
| `plugins-http-ai-guard` | 5 | 1 | 1 | 0 | pending |
| `plugins-http-ai-proxy` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-all-endpoint-status` | 3 | 1 | 1 | 0 | pending |
| `plugins-http-bandwidth-limit` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-basic-auth` | 3 | 1 | 2 | 0 | complete |
| `plugins-http-canary` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-common` | 13 | 0 | 0 | 0 | pending |
| `plugins-http-cors` | 2 | 1 | 1 | 0 | complete |
| `plugins-http-credential-injector` | 2 | 1 | 1 | 0 | complete |
| `plugins-http-csrf` | 3 | 3 | 0 | 0 | complete |
| `plugins-http-ctx-set` | 2 | 1 | 1 | 0 | complete |
| `plugins-http-direct-endpoint` | 2 | 1 | 1 | 0 | complete |
| `plugins-http-dsl` | 33 | 0 | 0 | 0 | pending |
| `plugins-http-dynamic-external-upstream` | 2 | 1 | 1 | 0 | complete |
| `plugins-http-dynamic-internal-upstream` | 2 | 1 | 1 | 0 | complete |
| `plugins-http-ext-proc` | 8 | 0 | 0 | 0 | pending |
| `plugins-http-fault-injection` | 2 | 1 | 1 | 0 | complete |
| `plugins-http-form-json-transform` | 2 | 1 | 1 | 0 | complete |
| `plugins-http-forward-auth` | 2 | 1 | 1 | 0 | complete |
| `plugins-http-framework` | 3 | 0 | 0 | 0 | pending |
| `plugins-http-geoip-location` | 2 | 1 | 1 | 0 | complete |
| `plugins-http-guardrail` | 5 | 1 | 4 | 0 | complete |
| `plugins-http-header-cert-auth` | 2 | 1 | 1 | 0 | complete |
| `plugins-http-hmac-auth` | 2 | 1 | 1 | 0 | complete |
| `plugins-http-ip-restriction` | 2 | 1 | 1 | 0 | complete |
| `plugins-http-json-schema-validation` | 2 | 1 | 1 | 0 | complete |
| `plugins-http-jwe-decrypt` | 2 | 0 | 0 | 0 | pending |
| `plugins-http-jwt-auth` | 15 | 0 | 0 | 0 | pending |
| `plugins-http-key-auth` | 2 | 1 | 1 | 0 | complete |
| `plugins-http-ldap-auth` | 2 | 1 | 1 | 0 | complete |
| `plugins-http-mock` | 2 | 1 | 1 | 0 | complete |
| `plugins-http-openid-connect` | 16 | 11 | 4 | 1 | complete |
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
| `plugins-http-webhook-key-get` | 2 | 1 | 1 | 0 | complete |
| `plugins-root` | 1 | 0 | 1 | 0 | complete |
| `plugins-runtime` | 35 | 12 | 3 | 1 | pending |
| `plugins-stream` | 18 | 3 | 0 | 0 | pending |
| `request-body` | 9 | 8 | 1 | 0 | complete |
| `response-processing` | 1 | 0 | 0 | 0 | pending |
| `routes-shared` | 66 | 1 | 1 | 0 | pending |
| `runtime` | 16 | 7 | 8 | 1 | complete |
| `services` | 4 | 1 | 3 | 0 | complete |
| `sse` | 3 | 1 | 2 | 0 | complete |
| `tls-boringssl` | 2 | 1 | 1 | 0 | complete |
| `tls-core` | 3 | 1 | 2 | 0 | complete |
| `tls-runtime` | 6 | 2 | 4 | 0 | complete |
| `tls-store` | 4 | 2 | 2 | 0 | complete |
| `tls-validation` | 6 | 3 | 2 | 1 | complete |
| `util` | 6 | 4 | 2 | 0 | complete |

## File ledger

| File | Batch | Kind | State | Change ID / note |
|------|-------|------|-------|------------------|
| `edgion-gateway/Cargo.toml` | `crate-root` | manifest | pending | — |
| `edgion-gateway/benches/lru_expiration.rs` | `crate-root` | benchmark | pending | Added by external commit `ab8074d0`; full audit pending |
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
| `edgion-gateway/src/api/mod.rs` | `api` | source | pending | Reopened after external 7534a9bf; prior B0168 evidence retained |
| `edgion-gateway/src/backends/circuit_breaker/concurrency_integration_tests.rs` | `backends-circuit-breaker` | test | changed | `B0033`: concurrent winner permits now prove RAII release to zero without leaking handles |
| `edgion-gateway/src/backends/circuit_breaker/mod.rs` | `backends-circuit-breaker` | source | examined-no-change | `B0033`: minimal production RAII wrapper retained |
| `edgion-gateway/src/backends/discovery/conf_handler_common.rs` | `backends-discovery-shared` | source | removed | `B0034` completed before external commit `8cbbed6c` removed the file |
| `edgion-gateway/src/backends/discovery/coordinator.rs` | `backends-discovery-shared` | source | pending | Added by external commit `c59a00e1`; full audit pending |
| `edgion-gateway/src/backends/discovery/dns_cache.rs` | `backends-discovery-shared` | source | deferred | `B0035`: D0001 query identity and Service lifecycle design |
| `edgion-gateway/src/backends/discovery/endpoint_slice/conf_handler_impl.rs` | `backends-discovery-endpoints` | source | deferred | `B0037`: D0002 ownership-transition cleanup lifecycle |
| `edgion-gateway/src/backends/discovery/endpoint_slice/discovery_impl.rs` | `backends-discovery-endpoints` | source | changed | `B0037`: removed request-path tracing from backend construction |
| `edgion-gateway/src/backends/discovery/endpoint_slice/ep_slice_store.rs` | `backends-discovery-endpoints` | source | deferred | `B0037`: D0002 same-key upsert and reverse-index ownership |
| `edgion-gateway/src/backends/discovery/endpoint_slice/mod.rs` | `backends-discovery-endpoints` | source | examined-no-change | `B0037`: minimal EndpointSlice module façade retained |
| `edgion-gateway/src/backends/discovery/mod.rs` | `backends-discovery-shared` | source | examined-no-change | `B0036`: minimal discovery façade and public re-exports retained |
| `edgion-gateway/src/backends/discovery/services/conf_handler_impl.rs` | `backends-discovery-services` | source | examined-no-change | `B0038`: serialized lifecycle and accepted cross-kind convergence retained |
| `edgion-gateway/src/backends/discovery/services/mod.rs` | `backends-discovery-services` | source | examined-no-change | `B0038`: minimal live façade retained |
| `edgion-gateway/src/backends/discovery/services/service_store.rs` | `backends-discovery-services` | source | examined-no-change | `B0038`: atomic ArcSwap snapshots and cold-path incremental clone retained |
| `edgion-gateway/src/backends/gateway_backend_tls_store.rs` | `backends-core` | source | deferred | `B0039`: D0003 authoritative Gateway identity replacement |
| `edgion-gateway/src/backends/health/check/config_store.rs` | `backends-health` | source | examined-no-change | `B0040`: cold single-writer snapshot store retained |
| `edgion-gateway/src/backends/health/check/manager.rs` | `backends-health` | source | changed | `B0040`: bounded probe-error classes; generation ownership deferred to D0004 |
| `edgion-gateway/src/backends/health/check/mod.rs` | `backends-health` | source | examined-no-change | `B0040`: narrow health-check module boundary retained |
| `edgion-gateway/src/backends/health/check/probes.rs` | `backends-health` | source | changed | `B0040`: HTTP redirects denied and fixed error classes added; TLS contract deferred to D0005 |
| `edgion-gateway/src/backends/health/check/status_store.rs` | `backends-health` | source | deferred | `B0040`: D0004 cancellation-safe task/status generation ownership |
| `edgion-gateway/src/backends/health/mod.rs` | `backends-health` | source | examined-no-change | `B0040`: minimal health façade retained |
| `edgion-gateway/src/backends/mod.rs` | `backends-core` | source | changed | `B0041`: borrowed ExternalName annotations remove a full hot-path map clone |
| `edgion-gateway/src/backends/outlier_detection/config.rs` | `backends-outlier` | source | examined-no-change | `B0042`: canonical schema-layer configuration re-export retained |
| `edgion-gateway/src/backends/outlier_detection/mod.rs` | `backends-outlier` | source | examined-no-change | `B0042`: coherent outlier module façade retained |
| `edgion-gateway/src/backends/outlier_detection/state.rs` | `backends-outlier` | source | changed | `B0042`: deterministic test-only deadline expiry removes wall-clock recovery sleeps |
| `edgion-gateway/src/backends/outlier_detection/store.rs` | `backends-outlier` | source | changed | `B0042`: deterministic recovery/backoff tests; detached gauge lifecycle deferred to D0006 |
| `edgion-gateway/src/backends/degrade_threshold.rs` | `backends-degrade-threshold` | source | pending | reopened after concurrent-agent content drift at `9830fb2a`; re-audit current HEAD |
| `edgion-gateway/src/backends/peer_builder.rs` | `backends-core` | source | changed | `B0043`: preserved complete upstream mTLS client chains and removed peer-construction allocations |
| `edgion-gateway/src/backends/policy/backend_tls/backend_tls_policy_store.rs` | `backends-policy-tls` | source | changed | BACKENDS-POLICY-TLS-001 / `f4acade9`, `c34041f3`; fail-close validation and true incremental publication deferred to dedicated TODOs |
| `edgion-gateway/src/backends/policy/backend_tls/conf_handler_impl.rs` | `backends-policy-tls` | source | examined-no-change | BACKENDS-POLICY-TLS-001: Arc-preserving count-only conf-sync adapter retained |
| `edgion-gateway/src/backends/policy/backend_tls/mod.rs` | `backends-policy-tls` | source | examined-no-change | BACKENDS-POLICY-TLS-001: narrow module exports retained |
| `edgion-gateway/src/backends/policy/backend_traffic/authority.rs` | `backends-policy-traffic` | source | examined-no-change | BACKENDS-POLICY-TRAFFIC-001: fail-close authority composition retained; bounded rendering deferred to dedicated TODO |
| `edgion-gateway/src/backends/policy/backend_traffic/conf_handler_impl.rs` | `backends-policy-traffic` | source | examined-no-change | BACKENDS-POLICY-TRAFFIC-001: ownership-required full-set clone and metadata-only logs retained |
| `edgion-gateway/src/backends/policy/backend_traffic/mod.rs` | `backends-policy-traffic` | source | examined-no-change | BACKENDS-POLICY-TRAFFIC-001: narrow exports retained; isolated singleton refcount change rejected |
| `edgion-gateway/src/backends/policy/backend_traffic/retry_budget.rs` | `backends-policy-traffic` | source | examined-no-change | BACKENDS-POLICY-TRAFFIC-001: atomic window/generation redesign deferred to dedicated TODO |
| `edgion-gateway/src/backends/policy/backend_traffic/store.rs` | `backends-policy-traffic` | source | pending | reopened after concurrent-agent content drift at `9830fb2a`; re-audit current HEAD |
| `edgion-gateway/src/backends/policy/mod.rs` | `backends-policy-root` | source | examined-no-change | BACKENDS-POLICY-ROOT-001: typed lookup boundary retained; unresolved Service section fail-close redesign deferred to dedicated TODO |
| `edgion-gateway/src/backends/port_resolver.rs` | `backends-port-resolver` | source | removed | BACKENDS-PORT-RESOLVER-001 completed before external commit `8cbbed6c` removed the file |
| `edgion-gateway/src/backends/preload.rs` | `backends-core` | source | removed | BACKENDS-PRELOAD-001 completed before external commit `8cbbed6c` removed the file |
| `edgion-gateway/src/backends/runtime.rs` | `backends-core` | source | changed | BACKENDS-RUNTIME-001: TLS-policy-safe selected-backend Debug boundary |
| `edgion-gateway/src/backends/selector.rs` | `backends-core` | source | pending | reopened after concurrent-agent content drift at `9830fb2a`; re-audit current HEAD |
| `edgion-gateway/src/backends/service_kind.rs` | `backends-core` | source | examined-no-change | BACKENDS-SERVICE-KIND-001: canonical resources-layer type re-export retained |
| `edgion-gateway/src/backends/tests/backend_tls_pool_test.rs` | `backends-core` | test | pending | Added by external commit `1ca0ad0e`; full audit pending |
| `edgion-gateway/src/backends/validation.rs` | `backends-core` | source | examined-no-change | BACKENDS-VALIDATION-001: DirectEndpoint membership/port proof redesign deferred to the shared backend-port TODO |
| `edgion-gateway/src/bin/edgion_gateway.rs` | `crate-entry` | source | examined-no-change | `B0044`: minimal allocator, panic, crypto, capability, and CLI handoff retained |
| `edgion-gateway/src/cache/lru.rs` | `cache` | source | changed | CACHE-001 / `40f7eb74` |
| `edgion-gateway/src/cache/mod.rs` | `cache` | source | examined-no-change | CACHE-001 / `40f7eb74` |
| `edgion-gateway/src/cache/sharded.rs` | `cache` | source | changed | CACHE-001 / `40f7eb74` |
| `edgion-gateway/src/cache/tiny_ufo.rs` | `cache` | source | examined-no-change | CACHE-001 / `40f7eb74` |
| `edgion-gateway/src/cli/config.rs` | `cli` | source | pending | — |
| `edgion-gateway/src/cli/mod.rs` | `cli` | source | pending | — |
| `edgion-gateway/src/cli/pingora.rs` | `cli` | source | pending | — |
| `edgion-gateway/src/conf_sync/cache_client/cache.rs` | `conf-sync-cache-client` | source | examined-no-change | CONF-SYNC-CACHE-001 / `21b01524` |
| `edgion-gateway/src/conf_sync/cache_client/cache_data.rs` | `conf-sync-cache-client` | source | examined-no-change | CONF-SYNC-CACHE-001 / `21b01524`; malformed-event recovery deferred to `tasks/todo/conf-sync-generic-watch-malformed-event-recovery.md` |
| `edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs` | `conf-sync-cache-client` | source | changed | CONF-SYNC-CACHE-001 / `21b01524` |
| `edgion-gateway/src/conf_sync/cache_client/mod.rs` | `conf-sync-cache-client` | source | examined-no-change | CONF-SYNC-CACHE-001 / `21b01524` |
| `edgion-gateway/src/conf_sync/conf_client/cert_watch.rs` | `conf-sync-conf-client` | source | examined-no-change | CONF-SYNC-CLIENT-001 / `9ca73353`; content-signature recovery accepted as `(mtime, length)` tradeoff in `skills/04-review/conf-sync/intentional-design/conf-sync-cert-watch-mtime-length-signature.md` |
| `edgion-gateway/src/conf_sync/conf_client/config_client.rs` | `conf-sync-conf-client` | source | examined-no-change | CONF-SYNC-CLIENT-001 / `9ca73353` |
| `edgion-gateway/src/conf_sync/conf_client/grpc_client.rs` | `conf-sync-conf-client` | source | changed | CONF-SYNC-CLIENT-001 / `9ca73353`; transactional rotation deferred to `tasks/todo/conf-sync-rotation-transactional-address-selection.md` |
| `edgion-gateway/src/conf_sync/conf_client/mod.rs` | `conf-sync-conf-client` | source | examined-no-change | CONF-SYNC-CLIENT-001 / `9ca73353` |
| `edgion-gateway/src/conf_sync/conf_client/rotation.rs` | `conf-sync-conf-client` | source | examined-no-change | CONF-SYNC-CLIENT-001 / `9ca73353`; transactional rotation deferred to `tasks/todo/conf-sync-rotation-transactional-address-selection.md` |
| `edgion-gateway/src/conf_sync/mod.rs` | `conf-sync-root` | source | examined-no-change | CONF-SYNC-ROOT-001 |
| `edgion-gateway/src/config/edgion_gateway/conf_handler_impl.rs` | `config-edgion-gateway` | source | pending | reopened after concurrent-agent content drift at `9830fb2a`; re-audit current HEAD |
| `edgion-gateway/src/config/edgion_gateway/mod.rs` | `config-edgion-gateway` | source | examined-no-change | CONFIG-001 / `4621d075` |
| `edgion-gateway/src/config/gateway_class/conf_handler_impl.rs` | `config-gateway-class` | source | examined-no-change | CONFIG-001 / `4621d075` |
| `edgion-gateway/src/config/gateway_class/mod.rs` | `config-gateway-class` | source | examined-no-change | CONFIG-001 / `4621d075` |
| `edgion-gateway/src/config/mod.rs` | `config-root` | source | examined-no-change | CONFIG-001 / `4621d075` |
| `edgion-gateway/src/ctx.rs` | `http-context` | source | pending | External ec5eb474..b1bea189 requires review; prior pending evidence retained |
| `edgion-gateway/src/geoip/config.rs` | `geoip` | source | changed | `B0059`: rejected enabled zero updater durations and byte cap |
| `edgion-gateway/src/geoip/info.rs` | `geoip` | source | examined-no-change | `B0059`: retained request-owned projection and IPv4-mapped normalization |
| `edgion-gateway/src/geoip/mod.rs` | `geoip` | source | examined-no-change | `B0059`: retained startup initialization, same-directory guard, and atomic provider publication |
| `edgion-gateway/src/geoip/provider.rs` | `geoip` | source | deferred | `B0059` / `D0025`: decode and panic failures require a cross-consumer failure-policy design |
| `edgion-gateway/src/geoip/updater.rs` | `geoip` | source | changed | `B0059`: protected Authorization metadata and made validator/304 recovery depend on usable local state |
| `edgion-gateway/src/lb/attempt.rs` | `lb-core` | source | pending | — |
| `edgion-gateway/src/lb/backend_selector/mod.rs` | `lb-backend-selector` | source | pending | — |
| `edgion-gateway/src/lb/backend_selector/selector.rs` | `lb-backend-selector` | source | pending | — |
| `edgion-gateway/src/lb/backend_selector/weighted_selector.rs` | `lb-backend-selector` | source | pending | — |
| `edgion-gateway/src/lb/compiled_selector.rs` | `lb-core` | source | pending | — |
| `edgion-gateway/src/lb/backend.rs` | `lb-core` | source | pending | — |
| `edgion-gateway/src/lb/ewma/score.rs` | `lb-ewma` | source | pending | Current tracked implementation after external commit `5c329cfe`; full audit pending |
| `edgion-gateway/src/lb/ewma/mod.rs` | `lb-ewma` | source | pending | — |
| `edgion-gateway/src/lb/leastconn/cleaner.rs` | `lb-leastconn` | source | pending | — |
| `edgion-gateway/src/lb/leastconn/mod.rs` | `lb-leastconn` | source | pending | — |
| `edgion-gateway/src/lb/mod.rs` | `lb-core` | source | pending | — |
| `edgion-gateway/src/lb/runtime_state/mod.rs` | `lb-runtime-state` | source | pending | — |
| `edgion-gateway/src/lb/sample.rs` | `lb-core` | source | pending | Added by external commit `c59a00e1`; full audit pending |
| `edgion-gateway/src/lb/selection/consistent_hash.rs` | `lb-selection` | source | pending | — |
| `edgion-gateway/src/lb/selection/mod.rs` | `lb-selection` | source | pending | — |
| `edgion-gateway/src/lb/selection/p2c.rs` | `lb-selection` | source | pending | — |
| `edgion-gateway/src/lb/selection/round_robin.rs` | `lb-selection` | source | pending | — |
| `edgion-gateway/src/lib.rs` | `crate-entry` | source | examined-no-change | `B0045`: compile guards, allocator test support, capability output, and root exports retained |
| `edgion-gateway/src/link_sys/mod.rs` | `link-sys-root` | source | changed | `B0046`: corrected LinkSys provider and crate-ownership documentation |
| `edgion-gateway/src/link_sys/providers/elasticsearch/bulk.rs` | `link-sys-provider-elasticsearch` | source | examined-no-change | `B0047`: bounded batching, retry, shutdown, and item-result behavior reviewed; D0007 tracks wider retry semantics |
| `edgion-gateway/src/link_sys/providers/elasticsearch/client.rs` | `link-sys-provider-elasticsearch` | source | examined-no-change | `B0047`: canonical outbound client, lifecycle, and health behavior retained |
| `edgion-gateway/src/link_sys/providers/elasticsearch/config_mapping.rs` | `link-sys-provider-elasticsearch` | source | changed | `B0047`: clamped explicit zero flush interval away from Tokio panic |
| `edgion-gateway/src/link_sys/providers/elasticsearch/log_sender.rs` | `link-sys-provider-elasticsearch` | source | examined-no-change | `B0047`: non-blocking DataSender boundary retained |
| `edgion-gateway/src/link_sys/providers/elasticsearch/mod.rs` | `link-sys-provider-elasticsearch` | source | examined-no-change | `B0047`: module exports retained |
| `edgion-gateway/src/link_sys/providers/elasticsearch/ops.rs` | `link-sys-provider-elasticsearch` | source | examined-no-change | `B0047`: bounded request and response handling retained |
| `edgion-gateway/src/link_sys/providers/kafka/client.rs` | `link-sys-provider-kafka` | source | changed | `B0056`: reused the canonical rollback-safe health flip-log gate while retaining B0048 validation and producer semantics |
| `edgion-gateway/src/link_sys/providers/kafka/log_sender.rs` | `link-sys-provider-kafka` | source | examined-no-change | `B0048`: non-blocking Kafka DataSender boundary retained |
| `edgion-gateway/src/link_sys/providers/kafka/mod.rs` | `link-sys-provider-kafka` | source | examined-no-change | `B0048`: module exports retained |
| `edgion-gateway/src/link_sys/providers/local_file/data_sender_impl.rs` | `link-sys-provider-local-file` | source | changed | `B0049`: preserved the canonical base path across rotation, gated replacement on flush success, prevented archive overwrite, tracked attachment health, and debounced size-rotation failure logs |
| `edgion-gateway/src/link_sys/providers/local_file/mod.rs` | `link-sys-provider-local-file` | source | examined-no-change | `B0049`: retained the narrow provider facade and exports |
| `edgion-gateway/src/link_sys/providers/local_file/rotation.rs` | `link-sys-provider-local-file` | source | changed | `B0049`: made size-index parsing safe when base-name prefix and suffix overlap |
| `edgion-gateway/src/link_sys/providers/mod.rs` | `link-sys-providers-root` | source | examined-no-change | `B0050`: retained the accurate provider facade, shared Etcd ownership, and live canonical exports |
| `edgion-gateway/src/link_sys/providers/otlp/log_sender.rs` | `link-sys-provider-otlp` | source | changed | `B0055`: materialized method/path only at OTLP attribute construction while retaining B0051 sensitive-path masking |
| `edgion-gateway/src/link_sys/providers/otlp/mod.rs` | `link-sys-provider-otlp` | source | examined-no-change | `B0051`: retained the narrow OTLP sender facade and re-export |
| `edgion-gateway/src/link_sys/providers/redis/client.rs` | `link-sys-provider-redis` | source | examined-no-change | `B0052`: retained canonical client ownership, hot-reload publication, and lifecycle behavior |
| `edgion-gateway/src/link_sys/providers/redis/config_mapping.rs` | `link-sys-provider-redis` | source | changed | `B0052`: made `rediss://` transport TLS effective and rejected ambiguous mixed active transports |
| `edgion-gateway/src/link_sys/providers/redis/mod.rs` | `link-sys-provider-redis` | source | examined-no-change | `B0052`: retained the narrow Redis provider facade and exports |
| `edgion-gateway/src/link_sys/providers/redis/ops.rs` | `link-sys-provider-redis` | source | examined-no-change | `B0052`: retained command, lock, and canonical RedisLinkClient operation behavior |
| `edgion-gateway/src/link_sys/providers/webhook/client.rs` | `link-sys-provider-webhook` | source | changed | `B0053`: made Webhook publication a prepared, sequence-checked store transition instead of a spawned generic lifecycle |
| `edgion-gateway/src/link_sys/providers/webhook/manager.rs` | `link-sys-provider-webhook` | source | pending | External 9b787978..b297af6e requires review; prior changed: `B0053`: serialized source generations and made one-key/full-map replacement and retirement atomic |
| `edgion-gateway/src/link_sys/providers/webhook/mod.rs` | `link-sys-provider-webhook` | source | changed | `B0053`: removed the obsolete manager lifecycle adapter exports |
| `edgion-gateway/src/link_sys/providers/webhook/resolver.rs` | `link-sys-provider-webhook` | source | pending | External 9b787978..b297af6e requires review; prior examined-no-change: `B0053`: retained canonical service resolution and namespace behavior |
| `edgion-gateway/src/link_sys/providers/webhook/runtime.rs` | `link-sys-provider-webhook` | source | deferred | `B0053` / `D0019`: window rotation and time-source semantics require a wider limiter contract |
| `edgion-gateway/src/link_sys/readme` | `link-sys-root` | documentation | changed | `B0054`: replaced fictional TODO/configuration claims with the current typed provider, output, fallback, and backfill boundaries |
| `edgion-gateway/src/link_sys/runtime/conf_handler.rs` | `link-sys-runtime` | source | examined-no-change | `B0057`: retained the nonblocking ordered conf-sync adapter |
| `edgion-gateway/src/link_sys/runtime/data_sender.rs` | `link-sys-runtime` | source | changed | `B0055`: made method/path metadata a call-scoped borrowed view |
| `edgion-gateway/src/link_sys/runtime/health_probe.rs` | `link-sys-runtime` | source | changed | `B0056`: became the sole owner of rollback-safe five-second health transition log gating |
| `edgion-gateway/src/link_sys/runtime/mod.rs` | `link-sys-runtime` | source | examined-no-change | `B0057`: retained the live minimal module facade and visibility boundaries |
| `edgion-gateway/src/link_sys/runtime/store.rs` | `link-sys-runtime` | source | pending | External 9b787978..b297af6e requires review; prior changed: `B0053`: ordered full/partial dispatch before side effects and retired omitted, deleted, or replaced Webhook generations |
| `edgion-gateway/src/link_sys/template/engine.rs` | `link-sys-template` | source | deferred | `B0058` / `D0024`: canonical policy-aware evaluator migration requires a cross-consumer design |
| `edgion-gateway/src/link_sys/template/mod.rs` | `link-sys-template` | source | deferred | `B0058` / `D0024`: facade ownership must move only with the canonical evaluator design |
| `edgion-gateway/src/observe/access_log/backfill.rs` | `observe-access-log` | source | changed | OBS-ACCESS-001 / `9d99d213` |
| `edgion-gateway/src/observe/access_log/entry.rs` | `observe-access-log` | source | examined-no-change | OBS-ACCESS-001 / `9d99d213` |
| `edgion-gateway/src/observe/access_log/log_ext.rs` | `observe-access-log` | source | changed | OBS-ACCESS-001 / `9d99d213` |
| `edgion-gateway/src/observe/access_log/logger.rs` | `observe-access-log` | source | changed | `B0055`: preserved first-healthy delivery across object-safe borrowed metadata dispatch |
| `edgion-gateway/src/observe/access_log/mod.rs` | `observe-access-log` | source | examined-no-change | OBS-ACCESS-001 / `9d99d213` |
| `edgion-gateway/src/observe/access_log_store.rs` | `observe-core` | source | changed | OBS-CORE-001 / `4b1d27d5` |
| `edgion-gateway/src/observe/logs/buffer.rs` | `observe-protocol-logs` | source | changed | OBS-LOG-001 / `11d3e79c` |
| `edgion-gateway/src/observe/logs/logger_factory.rs` | `observe-protocol-logs` | source | changed | OBS-LOG-001 / `11d3e79c` |
| `edgion-gateway/src/observe/logs/mod.rs` | `observe-protocol-logs` | source | examined-no-change | OBS-LOG-001 / `11d3e79c` |
| `edgion-gateway/src/observe/logs/ssl_log.rs` | `observe-protocol-logs` | source | changed | OBS-LOG-001 / `11d3e79c` |
| `edgion-gateway/src/observe/logs/tcp_log.rs` | `observe-protocol-logs` | source | examined-no-change | OBS-LOG-001 / `11d3e79c` |
| `edgion-gateway/src/observe/logs/tls_log.rs` | `observe-protocol-logs` | source | examined-no-change | OBS-LOG-001 / `11d3e79c` |
| `edgion-gateway/src/observe/logs/udp_log.rs` | `observe-protocol-logs` | source | changed | OBS-LOG-001 / `11d3e79c` |
| `edgion-gateway/src/observe/metrics/api.rs` | `observe-metrics` | source | examined-no-change | OBS-METRICS-001 / `96f8ac6a` |
| `edgion-gateway/src/observe/metrics/mod.rs` | `observe-metrics` | source | examined-no-change | OBS-METRICS-001 / `96f8ac6a` |
| `edgion-gateway/src/observe/metrics/registry.rs` | `observe-metrics` | source | pending | External ec5eb474..9e3a276b requires review; prior changed: OBS-METRICS-001 / `96f8ac6a` |
| `edgion-gateway/src/observe/metrics/route_handles.rs` | `observe-metrics` | source | changed | OBS-METRICS-001 / `96f8ac6a` |
| `edgion-gateway/src/observe/metrics/test_metrics.rs` | `observe-metrics` | source | changed | OBS-METRICS-001 / `96f8ac6a` |
| `edgion-gateway/src/observe/mod.rs` | `observe-core` | source | examined-no-change | OBS-CORE-001 / `4b1d27d5` |
| `edgion-gateway/src/observe/request_diagnostics.rs` | `observe-core` | source | examined-no-change | OBS-CORE-001 / `4b1d27d5` |
| `edgion-gateway/src/plugins/common/mod.rs` | `plugins-common` | source | examined-no-change | `B0061`: retained correct entry-scope compilation and identity separation |
| `edgion-gateway/src/plugins/common/redis_rate_limiter/mod.rs` | `plugins-common` | source | changed | B0233: shallow Fred input clones retain NOSCRIPT fallback; independent LGTM, 426 focused tests |
| `edgion-gateway/src/plugins/common/redis_rate_limiter/scripts.rs` | `plugins-common` | source | changed | B0232: persisted fractional refill; independent plan/final LGTM, focused filters passed |
| `edgion-gateway/src/plugins/http/ai_guard/evaluator.rs` | `plugins-http-ai-guard` | source | pending | External 9b787978..b297af6e requires review; prior changed: B0234: borrowed evaluator serialization spans; independent LGTM, evaluator 6/6 and module 30/30 |
| `edgion-gateway/src/plugins/http/ai_guard/mod.rs` | `plugins-http-ai-guard` | source | examined-no-change | B0235: minimal private-module facade retained; independent LGTM, AI Guard 30/30 |
| `edgion-gateway/src/plugins/http/ai_guard/plugin.rs` | `plugins-http-ai-guard` | source | changed | B0236: saturated physical-cap conversion, lazy refusal facts and borrowed evaluator projection; independent plan/final LGTM, AI Guard 32/32 |
| `edgion-gateway/src/plugins/http/ai_guard/rules.rs` | `plugins-http-ai-guard` | source | changed | B0237: bounded capture expansion and live replacement accounting; independent revised-plan/final LGTM, AI Guard 40/40 |
| `edgion-gateway/src/plugins/http/ai_guard/spans.rs` | `plugins-http-ai-guard` | source | changed | B0238: decoded-bound correctness, duplicate-part fail-close and bounded discriminator decoding; independent plan/final LGTM, AI Guard 49/49 |
| `edgion-gateway/src/plugins/http/ai_proxy/mod.rs` | `plugins-http-ai-proxy` | source | examined-no-change | B0064 |
| `edgion-gateway/src/plugins/http/ai_proxy/plugin.rs` | `plugins-http-ai-proxy` | source | pending | Reopened after external `bea47373` removed undeclared dye producers; B0064 history retained |
| `edgion-gateway/src/plugins/http/all_endpoint_status/mod.rs` | `plugins-http-all-endpoint-status` | source | examined-no-change | B0063 |
| `edgion-gateway/src/plugins/http/all_endpoint_status/plugin.rs` | `plugins-http-all-endpoint-status` | source | changed | B0063 |
| `edgion-gateway/src/plugins/http/all_endpoint_status/transport.rs` | `plugins-http-all-endpoint-status` | source | examined-no-change | B0239: request/endpoint isolation and canonical egress/TLS/bounds retained; independent LGTM, focused tests 18/18 |
| `edgion-gateway/src/plugins/http/bandwidth_limit/mod.rs` | `plugins-http-bandwidth-limit` | source | examined-no-change | B0065 |
| `edgion-gateway/src/plugins/http/bandwidth_limit/plugin.rs` | `plugins-http-bandwidth-limit` | source | changed | B0065 |
| `edgion-gateway/src/plugins/http/basic_auth/htpasswd.rs` | `plugins-http-basic-auth` | source | examined-no-change | B0067 |
| `edgion-gateway/src/plugins/http/basic_auth/mod.rs` | `plugins-http-basic-auth` | source | examined-no-change | B0067 |
| `edgion-gateway/src/plugins/http/basic_auth/plugin.rs` | `plugins-http-basic-auth` | source | changed | B0067 |
| `edgion-gateway/src/plugins/http/canary/mod.rs` | `plugins-http-canary` | source | examined-no-change | B0066 |
| `edgion-gateway/src/plugins/http/canary/plugin.rs` | `plugins-http-canary` | source | changed | B0066 |
| `edgion-gateway/src/plugins/http/common/auth_common.rs` | `plugins-http-common` | source | deferred | B0240: deterministic bounded JWT/OIDC/JWE projection remains owned by expanded D0052; independent approval |
| `edgion-gateway/src/plugins/http/common/capped_writer.rs` | `plugins-http-common` | source | examined-no-change | B0241: minimal atomic capped writer retained; independent LGTM, four consumer filters 27/27 |
| `edgion-gateway/src/plugins/http/common/http_client.rs` | `plugins-http-common` | source | changed | B0102: canonical default and dedicated RequestMirror singleton clients |
| `edgion-gateway/src/plugins/http/common/http_util.rs` | `plugins-http-common` | source | examined-no-change | B0242: strict bounded JSON reader retained; dynamic OIDC errors remain owned by existing P1 task; independent LGTM |
| `edgion-gateway/src/plugins/http/common/json_request.rs` | `plugins-http-common` | source | changed | B0243: borrowed raw HeaderMap classification removes per-request header clones; independent LGTM and focused tests passed |
| `edgion-gateway/src/plugins/http/common/bounded_regex_replacement.rs` | `plugins-http-common` | source | changed | B0253: canonical bounded regex replacement shared by DSL and AI Guard; independent plan/final LGTM |
| `edgion-gateway/src/plugins/http/common/mod.rs` | `plugins-http-common` | source | changed | B0244/B0253: rate-limit fast paths and canonical bounded regex helper export; independent reviews passed |
| `edgion-gateway/src/plugins/http/common/outbound/call.rs` | `plugins-http-common` | source | pending | External 9b787978..b297af6e requires review; prior changed: B0245: move sole-attempt requests and use monotonic passive recovery; independent revised-plan/final approval |
| `edgion-gateway/src/plugins/http/common/outbound/client.rs` | `plugins-http-common` | source | changed | B0246: fail-closed active-health URL/dial validation and correct passive/active circuit ownership |
| `edgion-gateway/src/plugins/http/common/outbound/egress.rs` | `plugins-http-common` | source | changed | B0247: saturating TTL publication and elapsed-time DNS cache expiry remove overflow/panic paths |
| `edgion-gateway/src/plugins/http/common/outbound/mod.rs` | `plugins-http-common` | source | examined-no-change | B0248: refreshed minimal coherent facade retained; independent LGTM rejected visibility/re-export churn |
| `edgion-gateway/src/plugins/http/common/outbound/pinned_resolver.rs` | `plugins-http-common` | source | changed | B0246: filter every pinned EndpointSlice address through canonical final-dial validation before rotation |
| `edgion-gateway/src/plugins/http/common/outbound/request.rs` | `plugins-http-common` | source | pending | External 9b787978..b297af6e requires review; prior changed: B0107: canonical framing helper reused by Wasm callouts |
| `edgion-gateway/src/plugins/http/common/outbound/target.rs` | `plugins-http-common` | source | changed | B0249: canonical Service section maps configured Service ports to actual ready serving sockets |
| `edgion-gateway/src/plugins/http/conf_handler_impl.rs` | `plugins-http-framework` | source | changed | B0062 |
| `edgion-gateway/src/plugins/http/cors/mod.rs` | `plugins-http-cors` | source | examined-no-change | B0068 |
| `edgion-gateway/src/plugins/http/cors/plugin.rs` | `plugins-http-cors` | source | changed | B0068 |
| `edgion-gateway/src/plugins/http/credential_injector/mod.rs` | `plugins-http-credential-injector` | source | examined-no-change | B0069 |
| `edgion-gateway/src/plugins/http/credential_injector/plugin.rs` | `plugins-http-credential-injector` | source | changed | B0069 |
| `edgion-gateway/src/plugins/http/csrf/mod.rs` | `plugins-http-csrf` | source | changed | B0070 |
| `edgion-gateway/src/plugins/http/csrf/plugin.rs` | `plugins-http-csrf` | source | changed | B0070 |
| `edgion-gateway/src/plugins/http/csrf/token.rs` | `plugins-http-csrf` | source | changed | B0070 |
| `edgion-gateway/src/plugins/http/ctx_set/mod.rs` | `plugins-http-ctx-set` | source | examined-no-change | B0071 |
| `edgion-gateway/src/plugins/http/ctx_set/plugin.rs` | `plugins-http-ctx-set` | source | changed | B0071 |
| `edgion-gateway/src/plugins/http/direct_endpoint/mod.rs` | `plugins-http-direct-endpoint` | source | examined-no-change | B0072 |
| `edgion-gateway/src/plugins/http/direct_endpoint/plugin.rs` | `plugins-http-direct-endpoint` | source | changed | B0072 |
| `edgion-gateway/src/plugins/http/dsl/config.rs` | `plugins-http-dsl` | source | changed | B0250: corrected stale schema/compiler/VM ownership documentation; independent LGTM |
| `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs` | `plugins-http-dsl` | source | changed | B0251: atomic bounded http.call failure, allocation-safe decoding and retained-header bounds; corrected independent LGTM, DSL 350/350 |
| `edgion-gateway/src/plugins/http/dsl/lang/mod.rs` | `plugins-http-dsl` | source | changed | B0252: removed three unused compatibility re-exports; independent LGTM |
| `edgion-gateway/src/plugins/http/dsl/lang/vm.rs` | `plugins-http-dsl` | source | changed | B0253: bounded regex construction before maxStringLen overflow; independent plan/final LGTM |
| `edgion-gateway/src/plugins/http/dsl/mod.rs` | `plugins-http-dsl` | source | changed | B0254: corrected compile/validation/runtime ownership rustdoc; independent LGTM |
| `edgion-gateway/src/plugins/http/dsl/plugin.rs` | `plugins-http-dsl` | source | changed | B0255: canonical 36-byte plugin-name validation and constructor closure; corrected independent LGTM |
| `edgion-gateway/src/plugins/http/dsl/runtime.rs` | `plugins-http-dsl` | source | changed | B0256: corrected per-generation runtime and owning knowledge descriptions; independent LGTM |
| `edgion-gateway/src/plugins/http/dsl/tests/api_errors.rs` | `plugins-http-dsl` | test | changed | B0257: completed all session-mutation API-error label coverage; independent LGTM |
| `edgion-gateway/src/plugins/http/dsl/tests/body_value.rs` | `plugins-http-dsl` | test | changed | B0258: locked dynamic-empty body.replace rejection before scan/mutation; independent LGTM |
| `edgion-gateway/src/plugins/http/dsl/tests/bytecode_deserialization.rs` | `plugins-http-dsl` | test | changed | B0259: removed stale version pollution from hostile validator fixtures; independent LGTM |
| `edgion-gateway/src/plugins/http/dsl/tests/compile_errors.rs` | `plugins-http-dsl` | test | changed | B0260: corrected false-positive inputs and pinned all intended diagnostics; independent LGTM |
| `edgion-gateway/src/plugins/http/dsl/tests/control_flow.rs` | `plugins-http-dsl` | test | changed | B0261: preserved loop-counter cleanup across inner break; independent LGTM |
| `edgion-gateway/src/plugins/http/dsl/tests/ctx_builtins.rs` | `plugins-http-dsl` | test | changed | B0262: locked ctx.get string limits and corrected response-phase evidence wording; independent LGTM |
| `edgion-gateway/src/plugins/http/dsl/tests/decision_log_tests.rs` | `plugins-http-dsl` | test | changed | B0263: removed legacy per-entry decision-log delimiters after corrected independent review |
| `edgion-gateway/src/plugins/http/dsl/tests/error_policy.rs` | `plugins-http-dsl` | test | changed | B0264: locked panic isolation and BodyAccess fixed-fail-close dye/log behavior; independent LGTM |
| `edgion-gateway/src/plugins/http/dsl/tests/grpc_error_tests.rs` | `plugins-http-dsl` | test | changed | B0265: locked nil-message translation and both invalid integer-code boundaries; independent LGTM |
| `edgion-gateway/src/plugins/http/dsl/tests/http_call_tests.rs` | `plugins-http-dsl` | test | changed | B0266: locked the physical request-body cap in opaque outbound calls before dialing; independent LGTM |
| `edgion-gateway/src/plugins/http/dsl/tests/mod.rs` | `plugins-http-dsl` | test | changed | B0267: corrected current-thread and multi-thread test-runtime topology docs; independent LGTM |
| `edgion-gateway/src/plugins/http/dsl/tests/p3_utility_tests.rs` | `plugins-http-dsl` | test | changed | B0268: bounded Join/UUID regression coverage and 14 isolated sensitive-mode tests; independent PASS, full DSL 373/373 |
| `edgion-gateway/src/plugins/http/dsl/tests/property_bytecode.rs` | `plugins-http-dsl` | test | changed | B0269: assert valid-source compile/VM success and exact source-map lookup; independent PASS, properties 4/4 |
| `edgion-gateway/src/plugins/http/dsl/tests/range_and_string_ops.rs` | `plugins-http-dsl` | test | changed | B0270: typed negative assertions, Unicode split and range hard-ceiling coverage; independent PASS, DSL 376/376 |
| `edgion-gateway/src/plugins/http/dsl/tests/redis_builtins.rs` | `plugins-http-dsl` | test | changed | B0271: exact rate-limit result budget errors and success boundary; independent PASS, DSL 377/377 |
| `edgion-gateway/src/plugins/http/dsl/tests/req_metadata_tests.rs` | `plugins-http-dsl` | test | changed | B0272: metadata string-cap and nil/Content-Length boundary tests; independent PASS, DSL 383/383 |
| `edgion-gateway/src/plugins/http/dsl/tests/request_builtins.rs` | `plugins-http-dsl` | test | changed | B0273: exact header-name multiplicities and cap boundaries; independent PASS, DSL 386/386; authorized D0142 resolves doc guard |
| `edgion-gateway/src/plugins/http/dsl/tests/request_mutation.rs` | `plugins-http-dsl` | test | changed | B0274: six exact host-call tests also pin Nil results; fixture syntax corrected, independent re-review PASS; targeted 6/6, DSL 386/386; D0143 deferred |
| `edgion-gateway/src/plugins/http/dsl/tests/resp_p2_tests.rs` | `plugins-http-dsl` | test | changed | B0275: exact response names, prefix/byte limits and missing Content-Type; independent LGTM, DSL 389/389 and all gates passed |
| `edgion-gateway/src/plugins/http/dsl/tests/resp_set_status_tests.rs` | `plugins-http-dsl` | test | changed | B0276: accepted endpoints/Nil and typed narrowing/type errors; independent LGTM, DSL 392/392; D0144 documents separate snapshot drift |
| `edgion-gateway/src/plugins/http/dsl/tests/response_builtins.rs` | `plugins-http-dsl` | test | changed | B0277: scalar header byte bounds and exact Bool/Nil observations; independent LGTM, DSL 394/394; original ten cases retained |
| `edgion-gateway/src/plugins/http/dsl/tests/runtime_errors.rs` | `plugins-http-dsl` | test | examined-no-change | B0278: precise typed VM errors adequately covered; independent LGTM, 10/10 and gates passed; incidental D0145 deferred |
| `edgion-gateway/src/plugins/http/dsl/tests/sandbox.rs` | `plugins-http-dsl` | test | changed | B0279: exact loop/string/map errors retain Deny coverage; independent LGTM, 3 exact tests plus policy 9/9; D0145 remains deferred |
| `edgion-gateway/src/plugins/http/dsl/tests/terminate_tests.rs` | `plugins-http-dsl` | test | deferred | B0280: D0146 termination contract conflict independently confirmed; four VM assertions preserved and passing at 9e3a276b |
| `edgion-gateway/src/plugins/http/dsl/tests/utility_functions.rs` | `plugins-http-dsl` | test | changed | B0253: regex grammar, zero-width, exact/plus-one and amplification regressions |
| `edgion-gateway/src/plugins/http/dsl/tests/vm_defensive_tests.rs` | `plugins-http-dsl` | test | examined-no-change | B0281: seven effective VM defenses retained; independent LGTM and 7/7 tests; D0147 knowledge correction tracked separately |
| `edgion-gateway/src/plugins/http/dynamic_external_upstream/mod.rs` | `plugins-http-dynamic-external-upstream` | source | examined-no-change | B0073 |
| `edgion-gateway/src/plugins/http/dynamic_external_upstream/plugin.rs` | `plugins-http-dynamic-external-upstream` | source | changed | B0073 |
| `edgion-gateway/src/plugins/http/dynamic_internal_upstream/mod.rs` | `plugins-http-dynamic-internal-upstream` | source | examined-no-change | B0074 |
| `edgion-gateway/src/plugins/http/dynamic_internal_upstream/plugin.rs` | `plugins-http-dynamic-internal-upstream` | source | changed | B0074 |
| `edgion-gateway/src/plugins/http/ext_proc/attached.rs` | `plugins-http-ext-proc` | source | changed | B0282: share constructor-normalized override list; request-local state retained; independent plan/final LGTM and focused 12/12 tests |
| `edgion-gateway/src/plugins/http/ext_proc/client.rs` | `plugins-http-ext-proc` | source | changed | B0283: reject unsupported effective TLS identity before cache/channel; independent plan/final LGTM, 11 exact tests and workspace gates passed |
| `edgion-gateway/src/plugins/http/ext_proc/mod.rs` | `plugins-http-ext-proc` | source | examined-no-change | B0284: justified module exports and real nonnetwork processor seam; independent LGTM, exact consumer and all gates passed |
| `edgion-gateway/src/plugins/http/ext_proc/mutation.rs` | `plugins-http-ext-proc` | source | changed | B0285: preserve current raw query through canonical URI projection; independent plan/final LGTM, mutation 12/12 and workspace gates passed |
| `edgion-gateway/src/plugins/http/ext_proc/plugin.rs` | `plugins-http-ext-proc` | source | changed | B0286: byte-exact local responses across header and streamed paths; independent plan/final LGTM, 21/21 deterministic tests and workspace gates passed |
| `edgion-gateway/src/plugins/http/ext_proc/response_streamed.rs` | `plugins-http-ext-proc` | source | examined-no-change | B0287: bounded per-chunk phase/terminal policy retained; independent LGTM, 11/11 deterministic tests and workspace gates passed; B0288 owns trailer finding |
| `edgion-gateway/src/plugins/http/ext_proc/response_trailer.rs` | `plugins-http-ext-proc` | source | changed | B0288: cleanup preserves settled shared-owner decisions; independent plan/final LGTM, 14/14 deterministic tests and workspace gates passed |
| `edgion-gateway/src/plugins/http/ext_proc/streamed.rs` | `plugins-http-ext-proc` | source | examined-no-change | B0289: request streaming lifecycle retained after external body-path refresh; independent LGTM, 17/17 deterministic tests and workspace gates passed |
| `edgion-gateway/src/plugins/http/fault_injection/mod.rs` | `plugins-http-fault-injection` | source | examined-no-change | B0075 |
| `edgion-gateway/src/plugins/http/fault_injection/plugin.rs` | `plugins-http-fault-injection` | source | changed | B0075 |
| `edgion-gateway/src/plugins/http/form_json_transform/mod.rs` | `plugins-http-form-json-transform` | source | examined-no-change | B0076 |
| `edgion-gateway/src/plugins/http/form_json_transform/plugin.rs` | `plugins-http-form-json-transform` | source | changed | B0076 |
| `edgion-gateway/src/plugins/http/forward_auth/mod.rs` | `plugins-http-forward-auth` | source | examined-no-change | B0077 |
| `edgion-gateway/src/plugins/http/forward_auth/plugin.rs` | `plugins-http-forward-auth` | source | changed | B0290: preserve repeated selected denial headers; independent plan/final LGTM, 7/7 focused tests and workspace gates passed |
| `edgion-gateway/src/plugins/http/geoip_location/mod.rs` | `plugins-http-geoip-location` | source | examined-no-change | B0078 |
| `edgion-gateway/src/plugins/http/geoip_location/plugin.rs` | `plugins-http-geoip-location` | source | changed | B0078 |
| `edgion-gateway/src/plugins/http/guardrail/attached.rs` | `plugins-http-guardrail` | source | examined-no-change | B0079 |
| `edgion-gateway/src/plugins/http/guardrail/mod.rs` | `plugins-http-guardrail` | source | examined-no-change | B0079 |
| `edgion-gateway/src/plugins/http/guardrail/plugin.rs` | `plugins-http-guardrail` | source | changed | B0079 |
| `edgion-gateway/src/plugins/http/guardrail/protocol.rs` | `plugins-http-guardrail` | source | examined-no-change | B0079 |
| `edgion-gateway/src/plugins/http/guardrail/response.rs` | `plugins-http-guardrail` | source | changed | B0291: non-Hold rejection claims GuardrailRejected before rendering; corrected plan/final LGTM, 17/17 tests and workspace gates passed |
| `edgion-gateway/src/plugins/http/header_cert_auth/mod.rs` | `plugins-http-header-cert-auth` | source | examined-no-change | B0080 |
| `edgion-gateway/src/plugins/http/header_cert_auth/plugin.rs` | `plugins-http-header-cert-auth` | source | changed | B0080 |
| `edgion-gateway/src/plugins/http/hmac_auth/mod.rs` | `plugins-http-hmac-auth` | source | examined-no-change | B0081 |
| `edgion-gateway/src/plugins/http/hmac_auth/plugin.rs` | `plugins-http-hmac-auth` | source | changed | B0081 |
| `edgion-gateway/src/plugins/http/ip_restriction/mod.rs` | `plugins-http-ip-restriction` | source | examined-no-change | B0082 |
| `edgion-gateway/src/plugins/http/ip_restriction/plugin.rs` | `plugins-http-ip-restriction` | source | changed | B0082 |
| `edgion-gateway/src/plugins/http/json_schema_validation/mod.rs` | `plugins-http-json-schema-validation` | source | examined-no-change | B0083 |
| `edgion-gateway/src/plugins/http/json_schema_validation/plugin.rs` | `plugins-http-json-schema-validation` | source | changed | B0083 |
| `edgion-gateway/src/plugins/http/jwe_decrypt/mod.rs` | `plugins-http-jwe-decrypt` | source | examined-no-change | B0084 |
| `edgion-gateway/src/plugins/http/jwe_decrypt/plugin.rs` | `plugins-http-jwe-decrypt` | source | changed | B0084 |
| `edgion-gateway/src/plugins/http/jwt_auth/credentials.rs` | `plugins-http-jwt-auth` | source | changed | B0085 |
| `edgion-gateway/src/plugins/http/jwt_auth/mod.rs` | `plugins-http-jwt-auth` | source | examined-no-change | B0085 |
| `edgion-gateway/src/plugins/http/jwt_auth/plugin.rs` | `plugins-http-jwt-auth` | source | changed | B0085 |
| `edgion-gateway/src/plugins/http/jwt_auth/request_stage.rs` | `plugins-http-jwt-auth` | source | examined-no-change | B0085 |
| `edgion-gateway/src/plugins/http/jwt_auth/tests/algorithm.rs` | `plugins-http-jwt-auth` | test | examined-no-change | B0085 |
| `edgion-gateway/src/plugins/http/jwt_auth/tests/anonymous.rs` | `plugins-http-jwt-auth` | test | examined-no-change | B0085 |
| `edgion-gateway/src/plugins/http/jwt_auth/tests/claim_propagation.rs` | `plugins-http-jwt-auth` | test | examined-no-change | B0085 |
| `edgion-gateway/src/plugins/http/jwt_auth/tests/claim_validation.rs` | `plugins-http-jwt-auth` | test | changed | B0085 |
| `edgion-gateway/src/plugins/http/jwt_auth/tests/common.rs` | `plugins-http-jwt-auth` | test | examined-no-change | B0085 |
| `edgion-gateway/src/plugins/http/jwt_auth/tests/error_classification.rs` | `plugins-http-jwt-auth` | test | examined-no-change | B0085 |
| `edgion-gateway/src/plugins/http/jwt_auth/tests/failure_response.rs` | `plugins-http-jwt-auth` | test | changed | B0085 |
| `edgion-gateway/src/plugins/http/jwt_auth/tests/mod.rs` | `plugins-http-jwt-auth` | test | examined-no-change | B0085 |
| `edgion-gateway/src/plugins/http/jwt_auth/tests/secret_decoding.rs` | `plugins-http-jwt-auth` | test | examined-no-change | B0085 |
| `edgion-gateway/src/plugins/http/jwt_auth/tests/token_source.rs` | `plugins-http-jwt-auth` | test | examined-no-change | B0085 |
| `edgion-gateway/src/plugins/http/jwt_auth/verify.rs` | `plugins-http-jwt-auth` | source | changed | B0085 |
| `edgion-gateway/src/plugins/http/key_auth/mod.rs` | `plugins-http-key-auth` | source | examined-no-change | B0087 |
| `edgion-gateway/src/plugins/http/key_auth/plugin.rs` | `plugins-http-key-auth` | source | changed | B0087 |
| `edgion-gateway/src/plugins/http/ldap_auth/mod.rs` | `plugins-http-ldap-auth` | source | examined-no-change | B0088 |
| `edgion-gateway/src/plugins/http/ldap_auth/plugin.rs` | `plugins-http-ldap-auth` | source | changed | B0088 |
| `edgion-gateway/src/plugins/http/mock/mod.rs` | `plugins-http-mock` | source | examined-no-change | B0089 |
| `edgion-gateway/src/plugins/http/mock/plugin.rs` | `plugins-http-mock` | source | changed | B0089 |
| `edgion-gateway/src/plugins/http/mod.rs` | `plugins-http-framework` | source | examined-no-change | B0062 |
| `edgion-gateway/src/plugins/http/openid_connect/auth_response_caps_integration.rs` | `plugins-http-openid-connect` | test | changed | B0292: stale task pointer replaced by durable D0036 knowledge; independent LGTM and workspace gates passed, eight ignored network tests unchanged/not run |
| `edgion-gateway/src/plugins/http/openid_connect/callback.rs` | `plugins-http-openid-connect` | source | examined-no-change | B0293: callback/state/PKCE audit retained; canonical supporting sanitizer now rejects browser authority-replacing backslashes, independent plan/final LGTM and exact test 1/1 |
| `edgion-gateway/src/plugins/http/openid_connect/config_validation.rs` | `plugins-http-openid-connect` | source | changed | B0094: borrowed claim scopes and allocation-free issuer selection |
| `edgion-gateway/src/plugins/http/openid_connect/discovery_jwks.rs` | `plugins-http-openid-connect` | source | changed | B0092 |
| `edgion-gateway/src/plugins/http/openid_connect/error.rs` | `plugins-http-openid-connect` | source | deferred | B0294: D0058 owns the cross-endpoint typed error/public-body/discovery-log redaction repair; independent review and adjudication confirmed no target-only fix |
| `edgion-gateway/src/plugins/http/openid_connect/jwks_provider.rs` | `plugins-http-openid-connect` | source | changed | B0295: anchor JWKS TTL and stale retry deadlines at atomic publication completion; independent plan/final LGTM and exact test 1/1 |
| `edgion-gateway/src/plugins/http/openid_connect/jwt_common.rs` | `plugins-http-openid-connect` | source | changed | B0090 |
| `edgion-gateway/src/plugins/http/openid_connect/logout.rs` | `plugins-http-openid-connect` | source | examined-no-change | B0093 |
| `edgion-gateway/src/plugins/http/openid_connect/mod.rs` | `plugins-http-openid-connect` | source | examined-no-change | B0094: minimal live module facade and visibility boundary retained |
| `edgion-gateway/src/plugins/http/openid_connect/plugin.rs` | `plugins-http-openid-connect` | source | examined-no-change | B0296: external change is cfg(test)-only registration; production construction/runtime invariants independently revalidated |
| `edgion-gateway/src/plugins/http/openid_connect/refresh_flight_registry.rs` | `plugins-http-openid-connect` | source | examined-no-change | B0092 |
| `edgion-gateway/src/plugins/http/openid_connect/request_filter_impl.rs` | `plugins-http-openid-connect` | source | changed | B0297: invalid/provider-error callbacks now emit a static diagnostic and fixed 400 instead of re-entering authentication; independent plan/final LGTM |
| `edgion-gateway/src/plugins/http/openid_connect/session_cookie.rs` | `plugins-http-openid-connect` | source | changed | B0091 |
| `edgion-gateway/src/plugins/http/openid_connect/tests.rs` | `plugins-http-openid-connect` | test | deferred | B0298/D0154: 24 load-bearing default-run OIDC JWKS tests across tests.rs/jwks_provider.rs use real loopback servers; isolate without deleting coverage |
| `edgion-gateway/src/plugins/http/openid_connect/token_exchange.rs` | `plugins-http-openid-connect` | source | pending | Reopened after external `74e6ccdd`; historical CACHE-001/B0091/B0093 retained |
| `edgion-gateway/src/plugins/http/openid_connect/token_verify.rs` | `plugins-http-openid-connect` | source | pending | Reopened after external `74e6ccdd`; historical B0090/B0091 retained |
| `edgion-gateway/src/plugins/http/openid_connect/userinfo.rs` | `plugins-http-openid-connect` | source | examined-no-change | B0093 |
| `edgion-gateway/src/plugins/http/plugin_store.rs` | `plugins-http-framework` | source | pending | External ec5eb474..9e3a276b requires review; prior pending: Reopened after external `5cb0b087` cross-namespace mirror LKG policy change; B0062/B0121 history retained |
| `edgion-gateway/src/plugins/http/proxy_rewrite/mod.rs` | `plugins-http-proxy-rewrite` | source | examined-no-change | B0095 |
| `edgion-gateway/src/plugins/http/proxy_rewrite/plugin.rs` | `plugins-http-proxy-rewrite` | source | changed | B0095 |
| `edgion-gateway/src/plugins/http/rate_limit/mod.rs` | `plugins-http-rate-limit` | source | examined-no-change | B0096 |
| `edgion-gateway/src/plugins/http/rate_limit/plugin.rs` | `plugins-http-rate-limit` | source | changed | B0096 |
| `edgion-gateway/src/plugins/http/rate_limit_redis/mod.rs` | `plugins-http-rate-limit-redis` | source | examined-no-change | B0097 |
| `edgion-gateway/src/plugins/http/rate_limit_redis/plugin.rs` | `plugins-http-rate-limit-redis` | source | pending | Reopened after external `264847df`; historical B0097 retained |
| `edgion-gateway/src/plugins/http/real_ip/mod.rs` | `plugins-http-real-ip` | source | examined-no-change | B0098 |
| `edgion-gateway/src/plugins/http/real_ip/plugin.rs` | `plugins-http-real-ip` | source | changed | B0098 |
| `edgion-gateway/src/plugins/http/region_route/log_consts.rs` | `plugins-http-region-route` | source | examined-no-change | B0099 |
| `edgion-gateway/src/plugins/http/region_route/mod.rs` | `plugins-http-region-route` | source | examined-no-change | B0099 |
| `edgion-gateway/src/plugins/http/region_route/plugin.rs` | `plugins-http-region-route` | source | changed | B0099 |
| `edgion-gateway/src/plugins/http/request_access_policy/mod.rs` | `plugins-http-request-access-policy` | source | examined-no-change | B0100 |
| `edgion-gateway/src/plugins/http/request_access_policy/plugin.rs` | `plugins-http-request-access-policy` | source | changed | B0100 |
| `edgion-gateway/src/plugins/http/request_body_buffer/mod.rs` | `plugins-http-request-body-buffer` | source | examined-no-change | B0101 |
| `edgion-gateway/src/plugins/http/request_body_buffer/plugin.rs` | `plugins-http-request-body-buffer` | source | examined-no-change | B0101 |
| `edgion-gateway/src/plugins/http/request_mirror/budget.rs` | `plugins-http-request-mirror` | source | pending | Added by external commit `90a45683`; full audit pending |
| `edgion-gateway/src/plugins/http/request_mirror/config.rs` | `plugins-http-request-mirror` | source | pending | Added by external commit `90a45683`; full audit pending |
| `edgion-gateway/src/plugins/http/request_mirror/mirror_log.rs` | `plugins-http-request-mirror` | source | examined-no-change | B0102 |
| `edgion-gateway/src/plugins/http/request_mirror/mirror_state.rs` | `plugins-http-request-mirror` | source | examined-no-change | B0102 |
| `edgion-gateway/src/plugins/http/request_mirror/mod.rs` | `plugins-http-request-mirror` | source | examined-no-change | B0102 |
| `edgion-gateway/src/plugins/http/request_mirror/plugin.rs` | `plugins-http-request-mirror` | source | pending | Reopened after external `5cb0b087` ReferenceGrant early rejection; B0102 history retained |
| `edgion-gateway/src/plugins/http/request_mirror/tests/plugin_test.rs` | `plugins-http-request-mirror` | test | pending | Added by external commit `90a45683`; full audit pending |
| `edgion-gateway/src/plugins/http/request_restriction/mod.rs` | `plugins-http-request-restriction` | source | examined-no-change | B0103 |
| `edgion-gateway/src/plugins/http/request_restriction/plugin.rs` | `plugins-http-request-restriction` | source | changed | B0103 |
| `edgion-gateway/src/plugins/http/response_rewrite/mod.rs` | `plugins-http-response-rewrite` | source | examined-no-change | B0104 |
| `edgion-gateway/src/plugins/http/response_rewrite/plugin.rs` | `plugins-http-response-rewrite` | source | changed | B0104 |
| `edgion-gateway/src/plugins/http/trace_context/mod.rs` | `plugins-http-trace-context` | source | examined-no-change | B0105 |
| `edgion-gateway/src/plugins/http/trace_context/plugin.rs` | `plugins-http-trace-context` | source | pending | Reopened after external 78a25c3a; prior B0105 evidence retained |
| `edgion-gateway/src/plugins/http/trace_context/w3c.rs` | `plugins-http-trace-context` | source | changed | B0105 |
| `edgion-gateway/src/plugins/http/waf/mod.rs` | `plugins-http-waf` | source | changed | B0106 |
| `edgion-gateway/src/plugins/http/wasm/callout.rs` | `plugins-http-wasm` | source | changed | B0107 |
| `edgion-gateway/src/plugins/http/wasm/host_abi.rs` | `plugins-http-wasm` | source | changed | B0108 |
| `edgion-gateway/src/plugins/http/wasm/mod.rs` | `plugins-http-wasm` | source | examined-no-change | B0109 |
| `edgion-gateway/src/plugins/http/wasm/oci/auth.rs` | `plugins-http-wasm` | source | changed | B0110 |
| `edgion-gateway/src/plugins/http/wasm/oci/cache.rs` | `plugins-http-wasm` | source | pending | Reopened after external 7534a9bf; prior B0111 evidence retained |
| `edgion-gateway/src/plugins/http/wasm/oci/cache_tests.rs` | `plugins-http-wasm` | test | pending | Added externally in 7534a9bf; requires campaign analysis |
| `edgion-gateway/src/plugins/http/wasm/oci/memory_pipeline_tests.rs` | `plugins-http-wasm` | test | pending | Added externally in 7534a9bf; requires campaign analysis |
| `edgion-gateway/src/plugins/http/wasm/oci/manifest.rs` | `plugins-http-wasm` | source | changed | B0112 |
| `edgion-gateway/src/plugins/http/wasm/oci/mod.rs` | `plugins-http-wasm` | source | pending | Reopened after external 7534a9bf; prior B0110 evidence retained |
| `edgion-gateway/src/plugins/http/wasm/plugin.rs` | `plugins-http-wasm` | source | changed | B0113 |
| `edgion-gateway/src/plugins/http/wasm/shared_data.rs` | `plugins-http-wasm` | source | changed | B0114 |
| `edgion-gateway/src/plugins/http/wasm/vm_manager.rs` | `plugins-http-wasm` | source | pending | Reopened after external 7534a9bf; prior B0115 evidence retained |
| `edgion-gateway/src/plugins/http/webhook_key_get/mod.rs` | `plugins-http-webhook-key-get` | source | examined-no-change | `B0116`: minimal private-module/public-type facade retained |
| `edgion-gateway/src/plugins/http/webhook_key_get/plugin.rs` | `plugins-http-webhook-key-get` | source | pending | External 9b787978..b297af6e requires review; prior changed: B0117 |
| `edgion-gateway/src/plugins/mod.rs` | `plugins-root` | source | examined-no-change | `B0118`: compile-time facade and live selective re-exports retained |
| `edgion-gateway/src/plugins/runtime/body_gate.rs` | `plugins-runtime` | source | changed | B0119 |
| `edgion-gateway/src/plugins/runtime/conditional_filter.rs` | `plugins-runtime` | source | changed | B0120 |
| `edgion-gateway/src/plugins/runtime/conditions/evaluator.rs` | `plugins-runtime` | source | changed | B0121 |
| `edgion-gateway/src/plugins/runtime/conditions/extract.rs` | `plugins-runtime` | source | examined-no-change | B0121 |
| `edgion-gateway/src/plugins/runtime/conditions/mod.rs` | `plugins-runtime` | source | examined-no-change | B0121 |
| `edgion-gateway/src/plugins/runtime/dye_plan.rs` | `plugins-runtime` | source | pending | D0083 resolved externally in `bea47373`; reopen changed file for current campaign audit; B0122 history retained |
| `edgion-gateway/src/plugins/runtime/edgion_config_data/conf_handler.rs` | `plugins-runtime` | source | changed | B0123: decision-scoped immutable snapshot capture and torn-generation regression |
| `edgion-gateway/src/plugins/runtime/edgion_config_data/mod.rs` | `plugins-runtime` | source | changed | B0123: crate-private snapshot API export |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/cors_filter.rs` | `plugins-runtime` | source | pending | External 9b787978..b297af6e requires review; prior changed: B0124: bounded inline CORS header pairs and validated-only construction |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/debug_access_log.rs` | `plugins-runtime` | source | changed | B0125: suppress raw Wrap-mode client exposure before snapshot construction |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs` | `plugins-runtime` | source | changed | B0126: shared precomputed ExtensionRef target and non-optional prepare success |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/header_modifier.rs` | `plugins-runtime` | source | changed | B0127: shared construction-time first-equivalent header plan |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/mod.rs` | `plugins-runtime` | source | examined-no-change | B0126: minimal Gateway API filter façade retained |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/path_modifier.rs` | `plugins-runtime` | source | changed | B0128: shared path-element replacement, root/trailing-slash and UTF-8 parameter regressions; verified |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/request_header_modifier.rs` | `plugins-runtime` | source | changed | B0127: borrowed prepared request-header operations with append/error regressions |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/request_redirect.rs` | `plugins-runtime` | source | changed | B0128: element-aware replacement and connection-derived redirect scheme; verified |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/response_header_modifier.rs` | `plugins-runtime` | source | changed | B0127: borrowed prepared response-header operations with append/error regressions |
| `edgion-gateway/src/plugins/runtime/gateway_api_filters/url_rewrite.rs` | `plugins-runtime` | source | changed | B0128: shared element-aware replacement preserving raw query and URI finalization; verified |
| `edgion-gateway/src/plugins/runtime/log.rs` | `plugins-runtime` | source | changed | B0129: bounded formatting/redaction and shared DSL strings; verified |
| `edgion-gateway/src/plugins/runtime/mod.rs` | `plugins-runtime` | source | examined-no-change | B0130: compile-time facade retained; independent review and gates passed |
| `edgion-gateway/src/plugins/runtime/plugin_runtime.rs` | `plugins-runtime` | source | pending | External ec5eb474..b1bea189 requires review; prior B0131/B0121 evidence retained |
| `edgion-gateway/src/plugins/runtime/policy_filter.rs` | `plugins-runtime` | source | examined-no-change | B0132: static Block/Bypass adapter retained after independent review |
| `edgion-gateway/src/plugins/runtime/reject_filter.rs` | `plugins-runtime` | source | changed | B0133: corrected fail-closed ownership and lenient-admission documentation |
| `edgion-gateway/src/plugins/runtime/session_adapter.rs` | `plugins-runtime` | source | pending | External b19975ed requires review; prior B0134 and REQ-BODY-CORE-001 evidence retained |
| `edgion-gateway/src/plugins/runtime/session_adapter/storage_io_tests.rs` | `plugins-runtime` | test | changed | B0135: independent watchdogs for deliberately blocked storage tests |
| `edgion-gateway/src/plugins/runtime/tests/body_gate_test.rs` | `plugins-runtime` | test | changed | `B0119`: externalized body-gate and compiled-policy coverage |
| `edgion-gateway/src/plugins/runtime/traits/header_ext.rs` | `plugins-runtime` | source | changed | B0136: exact per-name versus cross-name HeaderMap ordering contract |
| `edgion-gateway/src/plugins/runtime/traits/mod.rs` | `plugins-runtime` | source | examined-no-change | B0137: live layered facade and named lower-level paths retained |
| `edgion-gateway/src/plugins/runtime/traits/request_body_stream.rs` | `plugins-runtime` | source | changed | B0138: removed unused handler name API; supporting implementations retain separate coverage |
| `edgion-gateway/src/plugins/runtime/traits/request_filter.rs` | `plugins-runtime` | source | examined-no-change | B0139: object-safe conservative request-stage contract retained |
| `edgion-gateway/src/plugins/runtime/traits/session.rs` | `plugins-runtime` | source | changed | B0140: request-only capture and streaming-response contracts corrected |
| `edgion-gateway/src/plugins/runtime/traits/streaming_response_processor.rs` | `plugins-runtime` | source | changed | B0141: removed unused name API and repaired streaming design reference; support files retain separate coverage |
| `edgion-gateway/src/plugins/runtime/traits/test_helpers.rs` | `plugins-runtime` | source | examined-no-change | B0142: coherent strict test helpers retained |
| `edgion-gateway/src/plugins/runtime/traits/upstream_response.rs` | `plugins-runtime` | source | changed | B0143: correct final-response-header lifecycle contract |
| `edgion-gateway/src/plugins/runtime/traits/upstream_response_body_filter.rs` | `plugins-runtime` | source | changed | B0144: corrected data/terminal/trailer callback contract |
| `edgion-gateway/src/plugins/runtime/traits/upstream_response_filter.rs` | `plugins-runtime` | source | changed | B0145: corrected synchronous final-response-header contract |
| `edgion-gateway/src/plugins/runtime/traits/upstream_response_trailer.rs` | `plugins-runtime` | source | changed | B0146: corrected typed response-trailer companion contract |
| `edgion-gateway/src/plugins/stream/connection_filter_bridge.rs` | `plugins-stream` | source | pending | External d43d2539 requires review; prior B0147 evidence retained |
| `edgion-gateway/src/plugins/stream/connection_rate_limit/local_gate.rs` | `plugins-stream` | source | pending | External d43d2539 requires review; prior B0148 evidence retained |
| `edgion-gateway/src/plugins/stream/connection_rate_limit/mod.rs` | `plugins-stream` | source | examined-no-change | B0149: coherent zero-cost rate-limit module facade retained |
| `edgion-gateway/src/plugins/stream/connection_rate_limit/plugin.rs` | `plugins-stream` | source | pending | External ec5eb474..b1bea189 requires review; prior B0150 evidence retained |
| `edgion-gateway/src/plugins/stream/geoip_location/mod.rs` | `plugins-stream` | source | examined-no-change | B0151: minimal private-module/public-type GeoIP facade retained |
| `edgion-gateway/src/plugins/stream/geoip_location/plugin.rs` | `plugins-stream` | source | pending | External ec5eb474..9e3a276b requires review; prior changed: B0152: invalid generations fail closed; cached lookup need and exact deny causes |
| `edgion-gateway/src/plugins/stream/global_connection_ip_restriction/mod.rs` | `plugins-stream` | source | changed | B0153: corrected profile, Selector, lifecycle and DirectPeerIp facade docs |
| `edgion-gateway/src/plugins/stream/global_connection_ip_restriction/plugin.rs` | `plugins-stream` | source | changed | B0123: Selector and profile references share one snapshot generation |
| `edgion-gateway/src/plugins/stream/ip_restriction/mod.rs` | `plugins-stream` | source | examined-no-change | B0154: coherent crate-private implementation/public-type facade retained |
| `edgion-gateway/src/plugins/stream/ip_restriction/stream_ip_restriction.rs` | `plugins-stream` | source | changed | B0123: one lazy snapshot per referenced ACL decision |
| `edgion-gateway/src/plugins/stream/mod.rs` | `plugins-stream` | source | pending | External ec5eb474..9e3a276b requires review; prior changed: B0155: corrected connection-context and TLS-route runtime-family docs |
| `edgion-gateway/src/plugins/stream/stream_plugin_runtime.rs` | `plugins-stream` | source | pending | External d43d2539 requires review; prior B0156 evidence retained |
| `edgion-gateway/src/plugins/stream/stream_plugin_store.rs` | `plugins-stream` | source | pending | External ec5eb474..b1bea189 requires review; prior B0157 evidence retained |
| `edgion-gateway/src/plugins/stream/stream_plugin_trait.rs` | `plugins-stream` | source | pending | External ec5eb474..9e3a276b requires review; prior changed: B0158: corrected listener, TCP/UDP and legacy TLS lifecycle contract |
| `edgion-gateway/src/plugins/stream/tls_route/ip_restriction.rs` | `plugins-stream` | source | changed | B0123: one lazy snapshot per referenced ACL decision |
| `edgion-gateway/src/plugins/stream/tls_route/mod.rs` | `plugins-stream` | source | pending | External ec5eb474..9e3a276b requires review; prior changed: B0159: corrected terminated, passthrough and legacy fallback facade docs |
| `edgion-gateway/src/plugins/stream/tls_route/tls_route_plugin_runtime.rs` | `plugins-stream` | source | changed | B0160: borrowed Stage-2 metadata and removed discarded inner log state |
| `edgion-gateway/src/plugins/stream/tls_route/tls_route_plugin_trait.rs` | `plugins-stream` | source | changed | B0160: borrowed context contract, dead API removal and accurate dual TLS lifecycle docs |
| `edgion-gateway/src/request_body/body_scanners.rs` | `request-body` | source | changed | REQ-BODY-OPS-001 / `f36d7576`; REQ-BODY-BLOCKING-001 / `1eb48138` |
| `edgion-gateway/src/request_body/config.rs` | `request-body` | source | examined-no-change | REQ-BODY-CORE-001 / `a408d7fd` |
| `edgion-gateway/src/request_body/mod.rs` | `request-body` | source | changed | REQ-BODY-CORE-001 / `a408d7fd` |
| `edgion-gateway/src/request_body/observer.rs` | `request-body` | source | changed | REQ-BODY-OBSERVER-001 / `be461374` |
| `edgion-gateway/src/request_body/operation.rs` | `request-body` | source | changed | REQ-BODY-OPS-001 / `f36d7576` |
| `edgion-gateway/src/request_body/state.rs` | `request-body` | source | changed | REQ-BODY-CORE-001 / `a408d7fd` |
| `edgion-gateway/src/request_body/state/enabled.rs` | `request-body` | source | changed | REQ-BODY-CORE-001 / `a408d7fd`; REQ-BODY-BLOCKING-001 / `1eb48138` |
| `edgion-gateway/src/request_body/storage.rs` | `request-body` | source | changed | REQ-BODY-CORE-001 / `a408d7fd`; full audit REQ-BODY-STORAGE-001 / `ae283e46`; REQ-BODY-BLOCKING-001 / `1eb48138` |
| `edgion-gateway/src/request_body/storage_io_tests.rs` | `request-body` | test | changed | REQ-BODY-STORAGE-001 / `ae283e46` |
| `edgion-gateway/src/response_processor_driver.rs` | `response-processing` | source | examined-no-change | B0161: request-local processor ownership, cancellation leases and finalization retained |
| `edgion-gateway/src/routes/common.rs` | `routes-shared` | source | changed | B0162: filter delegation reverse-lock refs from listener attachment |
| `edgion-gateway/src/routes/grpc/conf_handler_impl.rs` | `routes-shared` | source | changed | B0163: share immutable rule-level metric handles across match units |
| `edgion-gateway/src/routes/grpc/grpc_timeout.rs` | `routes-shared` | source | changed | B0164: round encoded coarser timeout units upward |
| `edgion-gateway/src/routes/grpc/integration.rs` | `routes-shared` | source | changed | B0165 review retained the facade; B0167 borrows precompiled backend policy identity |
| `edgion-gateway/src/routes/grpc/match_engine.rs` | `routes-shared` | source | changed | B0166: correct regex service/method character precedence |
| `edgion-gateway/src/routes/grpc/match_unit.rs` | `routes-shared` | source | changed | B0167: precompile and borrow backend policy target identity |
| `edgion-gateway/src/routes/grpc/mod.rs` | `routes-shared` | source | changed | B0168: remove legacy singular gRPC manager alias |
| `edgion-gateway/src/routes/grpc/routes_mgr.rs` | `routes-shared` | source | changed | B0168 removed the legacy getter; B0169 clears retained managers and removes the inner ArcSwap |
| `edgion-gateway/src/routes/http/conf_handler_impl.rs` | `routes-shared` | source | pending | External 9b787978..b297af6e requires review; prior pending: Reopened after external `7825ef03` native CORS rule-scope enforcement; B0170 history retained |
| `edgion-gateway/src/routes/http/match_engine/mod.rs` | `routes-shared` | source | changed | B0171: enforce internal matcher construction boundary |
| `edgion-gateway/src/routes/http/match_engine/radix_path.rs` | `routes-shared` | source | changed | B0171: remove persistent fields exposed as dead by the internal boundary; B0172: move-only build value |
| `edgion-gateway/src/routes/http/match_engine/radix_route_match.rs` | `routes-shared` | source | changed | B0172: reduce route-table build allocations |
| `edgion-gateway/src/routes/http/match_engine/regex_routes_engine.rs` | `routes-shared` | source | changed | B0173: remove redundant regex-match index collection |
| `edgion-gateway/src/routes/http/match_unit.rs` | `routes-shared` | source | changed | B0172: expose borrowed zero-or-one path extraction |
| `edgion-gateway/src/routes/http/mod.rs` | `routes-shared` | source | changed | B0171: restrict the match engine to crate visibility |
| `edgion-gateway/src/routes/http/proxy_http/content_type.rs` | `routes-shared` | source | examined-no-change | B0174: retain zero-allocation protocol/media-type classification |
| `edgion-gateway/src/routes/http/proxy_http/global_plugins_resolve.rs` | `routes-shared` | source | changed | B0175: borrow the canonical plugin store and isolate resolver tests |
| `edgion-gateway/src/routes/http/proxy_http/mod.rs` | `routes-shared` | source | changed | B0176: retain only derived listener facts and default-disabled subrequests |
| `edgion-gateway/src/routes/http/proxy_http/parse_timeout.rs` | `routes-shared` | source | changed | B0177: preserve finite nonzero subsecond keepalive bounds |
| `edgion-gateway/src/routes/http/proxy_http/pg_connected_to_upstream.rs` | `routes-shared` | source | examined-no-change | B0178: SAN, attempt accounting, and immediate stream logging are already minimal and correct |
| `edgion-gateway/src/routes/http/proxy_http/pg_early_request_filter.rs` | `routes-shared` | source | changed | B0179: timeout/shutdown/framing retained; B0183: freeze raw protocol classification before Pingora module rewrite |
| `edgion-gateway/src/routes/http/proxy_http/pg_error_while_proxy.rs` | `routes-shared` | source | changed | B0180: preserve per-try timeout retry after non-terminal informational responses |
| `edgion-gateway/src/routes/http/proxy_http/pg_fail_to_connect.rs` | `routes-shared` | source | pending | External ec5eb474..9e3a276b requires review; prior changed: B0181: preserve typed deadline outcome after connection failure |
| `edgion-gateway/src/routes/http/proxy_http/pg_fail_to_proxy.rs` | `routes-shared` | source | changed | B0182: classify downstream read timeout as 408 without backend failure attribution |
| `edgion-gateway/src/routes/http/proxy_http/pg_init_downstream_modules.rs` | `routes-shared` | source | changed | B0183: keep the gRPC-Web bridge independent of frontend HTTP/2 enablement |
| `edgion-gateway/src/routes/http/proxy_http/pg_logging.rs` | `routes-shared` | source | changed | `B0055`: borrowed live method and canonical normalized path without per-request String allocation; response-processor finalization retained |
| `edgion-gateway/src/routes/http/proxy_http/pg_new_ctx.rs` | `routes-shared` | source | examined-no-change | OBS-METRICS-001 / `96f8ac6a`; B0176 support: consume derived listener port |
| `edgion-gateway/src/routes/http/proxy_http/pg_request_body_filter.rs` | `routes-shared` | source | pending | External b19975ed requires review; prior B0184 evidence retained |
| `edgion-gateway/src/routes/http/proxy_http/pg_request_body_filter_action.rs` | `routes-shared` | source | examined-no-change | B0185: retained ordered allocation-free body action dispatch after B0184 freshness review |
| `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs` | `routes-shared` | source | pending | External b19975ed requires review; prior B0175/B0176/B0183 evidence retained |
| `edgion-gateway/src/routes/http/proxy_http/pg_response_filter.rs` | `routes-shared` | source | changed | B0186: fail closed on unclaimable staged replies and skip disabled/SSE compression header scans |
| `edgion-gateway/src/routes/http/proxy_http/pg_response_head.rs` | `routes-shared` | source | examined-no-change | B0187: retained bounded response-head Hold/Release/Replace and final-writer ownership |
| `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs` | `routes-shared` | source | pending | External ec5eb474..b1bea189 requires review; prior B0188 evidence retained |
| `edgion-gateway/src/routes/http/proxy_http/pg_upstream_request_filter.rs` | `routes-shared` | source | changed | B0189: guard deferred credentials against Connection nomination and avoid backend attribution for local failures |
| `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_body_filter.rs` | `routes-shared` | source | changed | B0190: restore GRPCRoute and selected-backend read-only body-filter dispatch |
| `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_filter.rs` | `routes-shared` | source | examined-no-change | B0191: retained final/interim attempt accounting and response-plugin lifecycle |
| `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_trailer_filter.rs` | `routes-shared` | source | changed | B0192: prevent gateway-owned trailer failures from penalizing the selected backend |
| `edgion-gateway/src/routes/http/proxy_http/preflight_handler.rs` | `routes-shared` | source | examined-no-change | B0193: preflight ownership retained; external `7825ef03` closed D0084 through canonical rule-only CORS support |
| `edgion-gateway/src/routes/http/redirect_http.rs` | `routes-shared` | source | pending | External 9b787978..b297af6e requires review; prior changed: B0194: redact dedicated redirect access-log paths without changing the wire Location |
| `edgion-gateway/src/routes/http/routes_mgr.rs` | `routes-shared` | source | changed | B0169: clear exact retained manager on stale/orphan removal |
| `edgion-gateway/src/routes/http/session_persistence.rs` | `routes-shared` | source | changed | B0195: reuse multi-field cookie extraction and reject malformed expiry fallback |
| `edgion-gateway/src/routes/http/tests.rs` | `routes-shared` | test | changed | B0169: retained nonempty HTTP manager removal regressions |
| `edgion-gateway/src/routes/l4_duplex.rs` | `routes-shared` | source | changed | B0196: bounded independent directional progress under write/flush backpressure |
| `edgion-gateway/src/routes/l4_partial_update.rs` | `routes-shared` | source | examined-no-change | B0197: incremental listener ownership and per-listener publication verified |
| `edgion-gateway/src/routes/l4_rebuild.rs` | `routes-shared` | source | changed | B0169: shared L4 clear-on-remove invariant |
| `edgion-gateway/src/routes/mod.rs` | `routes-shared` | source | changed | B0168: remove legacy singular gRPC manager re-export |
| `edgion-gateway/src/routes/tcp/conf_handler_impl.rs` | `routes-shared` | source | changed | B0198: preserve immutable input schema Arcs across full/delta updates |
| `edgion-gateway/src/routes/tcp/edgion_tcp.rs` | `routes-shared` | source | changed | B0199: shorten table-guard ownership and remove address String clone |
| `edgion-gateway/src/routes/tcp/mod.rs` | `routes-shared` | source | examined-no-change | B0200: used exports and visibility retained without unnecessary churn |
| `edgion-gateway/src/routes/tcp/route_unit.rs` | `routes-shared` | source | examined-no-change | B0201: compiled unit ownership, selection and live plugin lookup verified |
| `edgion-gateway/src/routes/tcp/routes_mgr.rs` | `routes-shared` | source | changed | B0169: retained nonempty manager tests for shared L4 removal |
| `edgion-gateway/src/routes/tcp/tcp_route_table.rs` | `routes-shared` | source | examined-no-change | B0202: runtime retained; owning ListenerKey/order/compilation documentation corrected |
| `edgion-gateway/src/routes/tls/conf_handler_impl.rs` | `routes-shared` | source | changed | B0203: preserve incoming TLS schema Arcs through full/delta compilation |
| `edgion-gateway/src/routes/tls/gateway_tls_routes.rs` | `routes-shared` | source | changed | B0204: borrowed hostname inputs and winner-only published tables; independent LGTM, TLS 48/48 and routes 591/591 |
| `edgion-gateway/src/routes/tls/mod.rs` | `routes-shared` | source | pending | External 9b787978..b297af6e requires review; prior pending: External ec5eb474..9e3a276b requires review; prior deferred: B0205: D0115 coordinates Stage-1 fallback removal with frozen fixture migration; independent approval and fail-close 7/7 |
| `edgion-gateway/src/routes/tls/passthrough_proxy.rs` | `routes-shared` | source | pending | External 9b787978..b297af6e requires review; prior changed: B0206: released table and initial replay buffer before relay, borrowed endpoint; independent LGTM and routes 595/595 |
| `edgion-gateway/src/routes/tls/proxy.rs` | `routes-shared` | source | pending | External 9b787978..b297af6e requires review; prior changed: B0207: released table guard and avoided full TLS metadata copy; independent LGTM, routes 597/597; unrelated context assertions tracked D0119 |
| `edgion-gateway/src/routes/tls/route_unit.rs` | `routes-shared` | source | examined-no-change | B0208: preserved compiled immutable unit and shared selector/live plugin contracts; independent approval, unit 9/9 and TLS 54/54 |
| `edgion-gateway/src/routes/tls/routes_mgr.rs` | `routes-shared` | source | pending | Reopened after external 78a25c3a; prior B0209 serialized rebuild/cleanup evidence retained |
| `edgion-gateway/src/routes/tls/sni_parser.rs` | `routes-shared` | source | changed | B0210: enforced parent-bounded declared lengths; corrected independent LGTM, parser 16/16 and routes 602/602 |
| `edgion-gateway/src/routes/tls/tests.rs` | `routes-shared` | test | pending | Added by external f920df56; initial audit pending |
| `edgion-gateway/src/routes/udp/conf_handler_impl.rs` | `routes-shared` | source | changed | B0211: preserved schema Arcs through full/add/update; independent LGTM, handler 10/10 and routes 605/605 |
| `edgion-gateway/src/routes/udp/edgion_udp.rs` | `routes-shared` | source | changed | B0212: removed five session allocations and success-path IP formatting; independent LGTM, UDP 34/34; D0120–22 separate |
| `edgion-gateway/src/routes/udp/mod.rs` | `routes-shared` | source | deferred | B0213: independent approval of D0122 shutdown ownership task; UDP 34/34 |
| `edgion-gateway/src/routes/udp/route_unit.rs` | `routes-shared` | source | examined-no-change | B0214: runtime retained; owning plugin-key/inactivity knowledge corrected with independent LGTM |
| `edgion-gateway/src/routes/udp/routes_mgr.rs` | `routes-shared` | source | pending | Reopened after external 78a25c3a; prior B0215 publication/cleanup evidence retained |
| `edgion-gateway/src/routes/udp/udp_route_table.rs` | `routes-shared` | source | pending | Reopened after external 78a25c3a; prior B0216 lookup/route-count evidence retained |
| `edgion-gateway/src/runtime/gateway_info.rs` | `runtime` | source | changed | B0217: immutable precomputed borrowed identity key; independent LGTM and gates passed; local commit authorized |
| `edgion-gateway/src/runtime/handler.rs` | `runtime` | source | deferred | B0218: independently confirmed D0123/D0003, safe continuation; policy 12/12 and PGIS 10/10 |
| `edgion-gateway/src/runtime/matching/mod.rs` | `runtime` | source | examined-no-change | B0219: independently verified live facade; matching 41/41, no worthwhile rewrite |
| `edgion-gateway/src/runtime/matching/route.rs` | `runtime` | source | changed | B0220: reused parent-local Gateway lookup; independent LGTM, 19 route tests and adjacent matching gates passed |
| `edgion-gateway/src/runtime/matching/tests/tls_test.rs` | `runtime` | test | examined-no-change | B0221: independent LGTM; retained security tests, fresh 26/26; D0124 knowledge task |
| `edgion-gateway/src/runtime/matching/tls.rs` | `runtime` | source | pending | Reopened after external 78a25c3a; prior B0222 TLS evidence and D0125 investigation retained |
| `edgion-gateway/src/runtime/mod.rs` | `runtime` | source | examined-no-change | B0223: independent LGTM; retained explicit facade, identity 3/3 and matching 45/45 |
| `edgion-gateway/src/runtime/server/base.rs` | `runtime` | source | changed | B0176: remove the now-unconsumed GatewayClass clone chain |
| `edgion-gateway/src/runtime/server/error_response.rs` | `runtime` | source | changed | B0224: static HTML bytes, independent LGTM, response 11/11 and final status 8/8; D0126 separate |
| `edgion-gateway/src/runtime/server/listener_builder.rs` | `runtime` | source | changed | B0176: derive minimal immutable listener facts at construction |
| `edgion-gateway/src/runtime/server/mod.rs` | `runtime` | source | examined-no-change | B0225: independent LGTM; explicit writer/server facade retained; server tests 26/26 |
| `edgion-gateway/src/runtime/server/proxy_protocol_policy.rs` | `runtime` | source | examined-no-change | B0226: independent LGTM; policy 12/12, listener trust 1/1, CLI 2/2; frozen trust lifecycle retained |
| `edgion-gateway/src/runtime/server/server_header.rs` | `runtime` | source | changed | B0227: corrected three HSTS cost rustdoc lines only; independent LGTM and four response regressions passed |
| `edgion-gateway/src/runtime/store/gateway.rs` | `runtime` | source | examined-no-change | B0228: independent LGTM; raw store retained, owning API/frequency/snapshot knowledge corrected |
| `edgion-gateway/src/runtime/store/listener_gateway_info.rs` | `runtime` | source | changed | B0229: borrowed annotations without temporary maps; independent LGTM, store 14/14 and adjacent filters passed |
| `edgion-gateway/src/runtime/store/mod.rs` | `runtime` | source | examined-no-change | B0230: live facade exports retained; independent LGTM, store 14/14 and route 19/19 |
| `edgion-gateway/src/services/acme/challenge_store.rs` | `services` | source | changed | `B0060`: made the absolute expiry boundary exclusive with deterministic coverage |
| `edgion-gateway/src/services/acme/conf_handler_impl.rs` | `services` | source | examined-no-change | `B0060`: retained real incremental atomic publication and bounded config logging |
| `edgion-gateway/src/services/acme/mod.rs` | `services` | source | examined-no-change | `B0060`: retained the minimal live ACME facade and exports |
| `edgion-gateway/src/services/mod.rs` | `services` | source | examined-no-change | `B0060`: retained the minimal services root declaration |
| `edgion-gateway/src/sse/chain.rs` | `sse` | source | changed | SSE-CHAIN-001 / `4327da2b` |
| `edgion-gateway/src/sse/mod.rs` | `sse` | source | examined-no-change | SSE-CHAIN-001 decoder/framing audit; 62-test baseline preserved |
| `edgion-gateway/src/sse/window.rs` | `sse` | source | examined-no-change | SSE-WINDOW-001: confirmed complex findings deferred to `tasks/todo/sse-window-strict-residency-and-continuity.md`; no window code landed |
| `edgion-gateway/src/tls/boringssl/mod.rs` | `tls-boringssl` | source | examined-no-change | TLS-BORINGSSL-001 / `f33146ef` |
| `edgion-gateway/src/tls/boringssl/mtls_verify_callback.rs` | `tls-boringssl` | source | changed | TLS-BORINGSSL-001 / `f33146ef` |
| `edgion-gateway/src/tls/fips_host.rs` | `tls-core` | source | examined-no-change | TLS-CORE-001 / `9ab98fd0` |
| `edgion-gateway/src/tls/fips_profile.rs` | `tls-core` | source | changed | TLS-CORE-001 / `9ab98fd0` |
| `edgion-gateway/src/tls/mod.rs` | `tls-core` | source | examined-no-change | TLS-CORE-001 / `9ab98fd0` |
| `edgion-gateway/src/tls/runtime/backend/backend_api.rs` | `tls-runtime` | source | examined-no-change | TLS-RUNTIME-001 / `0090e5bf` |
| `edgion-gateway/src/tls/runtime/backend/cert_extractor.rs` | `tls-runtime` | source | changed | TLS-RUNTIME-001 / `0090e5bf` |
| `edgion-gateway/src/tls/runtime/backend/mod.rs` | `tls-runtime` | source | examined-no-change | TLS-RUNTIME-001 / `0090e5bf` |
| `edgion-gateway/src/tls/runtime/gateway/mod.rs` | `tls-runtime` | source | examined-no-change | TLS-RUNTIME-001 / `0090e5bf` |
| `edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs` | `tls-runtime` | source | pending | Reopened after external 78a25c3a; prior TLS-RUNTIME-001/0090e5bf and owned-handoff/exactly-once audit evidence retained |
| `edgion-gateway/src/tls/runtime/mod.rs` | `tls-runtime` | source | examined-no-change | TLS-RUNTIME-001 / `0090e5bf` |
| `edgion-gateway/src/tls/store/cert_matcher.rs` | `tls-store` | source | examined-no-change | TLS-STORE-001 / `19d6e382` |
| `edgion-gateway/src/tls/store/conf_handler.rs` | `tls-store` | source | changed | TLS-STORE-001 / `19d6e382` |
| `edgion-gateway/src/tls/store/mod.rs` | `tls-store` | source | examined-no-change | TLS-STORE-001 / `19d6e382` |
| `edgion-gateway/src/tls/store/tls_store.rs` | `tls-store` | source | changed | TLS-STORE-001 / `19d6e382`; full-snapshot identity and true incremental publication deferred to dedicated todos |
| `edgion-gateway/src/tls/validation/backend_san.rs` | `tls-validation` | source | changed | TLS-VALIDATION-001 / `f65f66bc` |
| `edgion-gateway/src/tls/validation/cert.rs` | `tls-validation` | source | examined-no-change | TLS-VALIDATION-001: canonical shared validation re-export retained |
| `edgion-gateway/src/tls/validation/mod.rs` | `tls-validation` | source | examined-no-change | TLS-VALIDATION-001: narrow module exports retained |
| `edgion-gateway/src/tls/validation/mtls.rs` | `tls-validation` | source | changed | TLS-VALIDATION-001 / `f65f66bc`; typed SAN semantics deferred to dedicated TODO |
| `edgion-gateway/src/tls/validation/outbound_client.rs` | `tls-validation` | source | deferred | B0231/D0127: per-bundle CA validation requires plan-first cross-adapter repair |
| `edgion-gateway/src/tls/validation/outbound_san_verifier.rs` | `tls-validation` | source | changed | TLS-VALIDATION-001 / `f65f66bc` |
| `edgion-gateway/src/util/client_ip.rs` | `util` | source | changed | UTIL-001 / `a909f16f` |
| `edgion-gateway/src/util/mod.rs` | `util` | source | examined-no-change | UTIL-001 / `a909f16f` |
| `edgion-gateway/src/util/net.rs` | `util` | source | changed | UTIL-001 / `a909f16f` |
| `edgion-gateway/src/util/proxy_protocol.rs` | `util` | source | changed | UTIL-001 / `a909f16f` |
| `edgion-gateway/src/util/real_ip_extractor.rs` | `util` | source | changed | UTIL-001 / `a909f16f`; support reopened in B0098 |
| `edgion-gateway/src/util/redacted.rs` | `util` | source | examined-no-change | UTIL-001 / `a909f16f` |
| `edgion-gateway/tests/conf_sync_failover.rs` | `external-tests` | test | pending | Added by external commit `715e20d5`; full audit pending |
| `edgion-gateway/tests/geoip_fixture_readback.rs` | `external-tests` | test | pending | — |
| `edgion-gateway/tests/tls_passthrough_clienthello.rs` | `external-tests` | test | pending | Added by external commit `55bb2c84`; full audit pending |
