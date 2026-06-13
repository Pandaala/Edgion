# Dead Code Candidates — Consolidated

Aggregated from 6 parallel Sonnet agent scans (730 .rs files total).
All items verified with grep-based reference counts during scanning.

---

## Category A — High Confidence: Compiler-Annotated, 0 Production Callers

These all carry `#[allow(dead_code)]` and have no callers outside tests/construction code.

### A1: Gateway

| File:Line | Item | Notes |
|-----------|------|-------|
| `edgion-gateway/src/lb/lb_policy/config.rs:26` | `fn parse_policies()` | No callers anywhere — REMOVED in refactor (not found 2026-06-13) |
| `edgion-gateway/src/lb/lb_policy/config.rs:32` | `fn parse_service_key()` | Private, no callers — REMOVED in refactor (not found 2026-06-13) |
| `edgion-gateway/src/link_sys/runtime/data_sender.rs:46` | `type StringDataSender` | Type alias, never referenced — REMOVED in refactor (not found 2026-06-13) |
| `edgion-gateway/src/plugins/http/dsl/lang/parser/mod.rs:85` | `fn Parser::span_at()` | Debug helper, no callers — REMOVED in refactor (not found 2026-06-13) |
| `edgion-gateway/src/runtime/server/error_response.rs:150` | `async fn end_response_403()` | 403 handled via plugins, not here — REMOVED in refactor (not found 2026-06-13) |
| `edgion-gateway/src/backends/health/check/config_store.rs:86` | `fn endpoint_slice_keys()` | No callers outside file — REMOVED in refactor (not found 2026-06-13) |
| `edgion-gateway/src/backends/health/check/config_store.rs:90` | `fn endpoint_keys()` | No callers outside file — REMOVED in refactor (not found 2026-06-13) |
| `edgion-gateway/src/backends/policy/backend_tls/backend_tls_policy_store.rs:282` | `fn get_all()` | No callers in production — REMOVED in refactor (not found 2026-06-13) |
| `edgion-gateway/src/backends/policy/backend_tls/backend_tls_policy_store.rs:287` | `fn get_by_namespace()` | No callers in production — REMOVED in refactor (not found 2026-06-13) |
| `edgion-gateway/src/tls/store/tls_store.rs:249` | `fn rebuild_matcher()` | Private, no callers — REMOVED in refactor (not found 2026-06-13) |

### A2: Controller

| File:Line | Item | Notes |
|-----------|------|-------|
| `edgion-controller/src/conf_sync/conf_server/grpc_server.rs:57` | `fn with_static_server()` | No callers, not wired into any binary — REMOVED in refactor (not found 2026-06-13) |
| `edgion-controller/src/conf_sync/conf_server/grpc_server.rs:160` | `async fn serve_with_reflection()` | Alternate entry point, not wired — REMOVED in refactor (not found 2026-06-13) |
| `edgion-controller/src/conf_sync/conf_server/client_registry.rs:23` | `ClientMeta::client_name` | Written, never read — REMOVED in refactor (not found 2026-06-13) |
| `edgion-controller/src/conf_sync/conf_server/client_registry.rs:25` | `ClientMeta::connected_at` | Written, never read — REMOVED in refactor (not found 2026-06-13) |
| `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handler.rs:145` | `struct DefaultHandler` | Only in `#[cfg(test)]` — REMOVED in refactor (not found 2026-06-13) |
| `edgion-common/src/secret/secret_store.rs:71` | `fn SecretStore::keys()` | No production callers — REMOVED in refactor (not found 2026-06-13; file moved to edgion-common, fn absent) |
| `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/backend_tls_policy.rs:595` | `fn secret_has_ca_cert()` | No callers anywhere — REMOVED in refactor (not found 2026-06-13) |
| `edgion-controller/src/api/types.rs:47` | `fn AdminState::is_ready()` | No production callers |

### A3: Common (not annotated but confirmed 0 callers anywhere)

| File:Line | Item | Notes |
|-----------|------|-------|
| `edgion-common/src/utils/net.rs:14` | `fn parse_optional_listen_addr()` | Never called, period — REMOVED in refactor (not found 2026-06-13) |
| `edgion-common/src/utils/net.rs:19` | `fn normalize_grpc_endpoint()` | Never called, period — REMOVED in refactor (not found 2026-06-13) |
| `edgion-common/src/utils/net.rs:27` | `fn default_operator_addr()` | Never called, period — REMOVED in refactor (not found 2026-06-13) |
| `edgion-common/src/utils/mod.rs:127` | `fn format_resource_info()` | Never called, period — REMOVED in refactor (not found 2026-06-13) |
| `edgion-resources/src/matcher/host_match/radix_match/radix_host_match.rs:51` | `fn export_hosts()` | Never called, period — REMOVED in refactor (not found 2026-06-13) |

### A4: CTL

| File:Line | Item | Notes |
|-----------|------|-------|
| `edgion-cli/src/cli/output.rs:93` | `fn print_message()` | 0 callers anywhere, leftover from refactor — REMOVED in refactor (not found 2026-06-13) |

### A5: Types — Constants Never Imported

| File:Line | Constant | Notes |
|-----------|----------|-------|
| `edgion-resources/src/constants/app.rs:10` | `LOG_PREFIX_OPERATOR` | Never imported anywhere — REMOVED in refactor (not found 2026-06-13) |
| `edgion-resources/src/constants/app.rs:13` | `LOG_PREFIX_GATEWAY` | Never imported anywhere — REMOVED in refactor (not found 2026-06-13) |
| `edgion-resources/src/constants/app.rs:16` | `DEFAULT_LOG_DIR` | Never imported anywhere — REMOVED in refactor (not found 2026-06-13) |
| `edgion-resources/src/constants/app.rs:19` | `DEFAULT_CONFIG_DIR` | Never imported anywhere — REMOVED in refactor (not found 2026-06-13) |
| `edgion-resources/src/constants/app.rs:22` | `DEFAULT_WORK_DIR` | Never imported anywhere — REMOVED in refactor (not found 2026-06-13) |
| `edgion-resources/src/constants/app.rs:4` | `CONTROLLER_NAME` | Only in a doc comment in same module |
| `edgion-resources/src/constants/annotations.rs:62` | `k8s::LAST_APPLIED_CONFIG` | Never imported anywhere — REMOVED in refactor (not found 2026-06-13) |
| `edgion-resources/src/constants/labels.rs:13` | `edgion::MANAGED_BY` | Never imported anywhere — REMOVED in refactor (not found 2026-06-13) |
| `edgion-resources/src/constants/headers.rs:8..46` | Entire `headers.rs` module (16 constants) | Code uses raw string literals; constants never imported — REMOVED in refactor (not found 2026-06-13) |

### A6: Types — Enums/Structs Never Instantiated in Production

| File:Line | Item | Notes |
|-----------|------|-------|
| `edgion-resources/src/filters.rs:65` | `enum PluginTags` | Never instantiated or matched anywhere — REMOVED in refactor (not found 2026-06-13) |

---

## Category B — Medium Confidence: Test-Only Usage, 0 Production Callers

These functions exist as public API but are only referenced in unit tests within their own file.

| File:Line | Item | Notes |
|-----------|------|-------|
| `edgion-gateway/src/util/net.rs:74` | `fn is_localhost()` | Only in tests; doc says "backward compat" but no callers |
| `edgion-common/src/utils/mod.rs:74` | `fn check_need_version()` | Only in unit tests within same file — REMOVED in refactor (not found 2026-06-13) |
| `edgion-resources/src/matcher/ip_radix_tree/types.rs:134` | `fn u32_to_ipv4()` | Only in unit tests |
| `edgion-resources/src/matcher/ip_radix_tree/types.rs:139` | `fn u128_to_ipv6()` | Only in unit tests |
| `edgion-resources/src/matcher/ip_radix_tree/matcher.rs:91` | `fn has_ipv4_rules()` | Only in unit tests |
| `edgion-resources/src/matcher/ip_radix_tree/matcher.rs:96` | `fn has_ipv6_rules()` | Only in unit tests |
| `edgion-resources/src/matcher/ip_radix_tree/matcher.rs:212` | `fn ipv4_rule_count()` | Only in unit tests |
| `edgion-resources/src/matcher/ip_radix_tree/matcher.rs:217` | `fn ipv6_rule_count()` | Only in unit tests |
| `edgion-resources/src/matcher/ip_radix_tree/matcher.rs:101` | `fn IpRadixMatcher::stats()` + `struct MatcherStats` | 0 external callers; stats infra unused |
| `edgion-resources/src/matcher/ip_radix_tree/frozen_v4.rs:142` | `fn FrozenIpV4RadixTree::stats()` | Called only by IpRadixMatcher::stats() |
| `edgion-resources/src/matcher/ip_radix_tree/frozen_v6.rs:142` | `fn FrozenIpV6RadixTree::stats()` | Called only by IpRadixMatcher::stats() |
| `edgion-resources/src/matcher/radix_tree/frozen.rs:548` | `fn FrozenRadixTree::match_exact()` | Only in unit tests |
| `edgion-resources/src/matcher/radix_tree/frozen.rs:567` | `fn FrozenRadixTree::stats()` + `TreeStats` | Never called from production |
| `edgion-resources/src/matcher/ip_radix_tree/types.rs:115,120,125` | `IpCidr::is_v4`, `is_v6`, `prefix_len` | Never called, direct pattern matching used instead |
| `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/status_utils.rs:404` | `fn accepted_condition_with_message()` | Only called by `accepted_condition()` in same file; should be inlined |
| `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/secret_utils/secret_ref.rs:15` | `fn new_secret_ref_manager()` | Only in `#[cfg(test)]` blocks |

---

## Category C — Unimplemented Feature Schema Types

These are type definitions for features declared in schema but never consumed by runtime code.

| File:Line | Item | Notes |
|-----------|------|-------|
| `edgion-resources/src/resources/edgion_tls.rs:192` | `struct EdgionTlsExtend` | Controller always writes `extend: None`; gateway never reads `.extend` |
| `edgion-resources/src/resources/edgion_tls.rs:115` | `struct OcspStaplingConfig` | Nested in EdgionTlsExtend, never consumed |
| `edgion-resources/src/resources/edgion_tls.rs:221` | `struct SessionTicketConfig` | Nested in EdgionTlsExtend, never consumed |
| `edgion-resources/src/resources/edgion_tls.rs:239` | `struct SessionCacheConfig` | Nested in EdgionTlsExtend, never consumed |
| `edgion-resources/src/resources/edgion_tls.rs:262` | `struct RevocationCheckConfig` + `enum RevocationMode` | Nested in EdgionTlsExtend, never consumed |
| `edgion-resources/src/resources/edgion_tls.rs:287` | `struct EarlyDataConfig` | Nested in EdgionTlsExtend, never consumed |
| `edgion-resources/src/resources/grpc_route.rs:339` | `struct GRPCRouteRetry` | GRPC retry field, no gateway handler reads `.retry` |
| `edgion-resources/src/resources/grpc_route.rs:305` | `struct GRPCRequestMirrorFilter` | GRPC mirror filter, no gateway handler processes it |
| `edgion-resources/src/resources/link_sys/webhook.rs:319` | `struct ActiveHealthCheck` | 0 external refs; only PassiveHealthCheck is used |
| `edgion-resources/src/resources/gateway_class.rs:62` | `struct ParametersReference` | Field type in GatewayClassSpec.parameters_ref, never named outside gateway_class.rs |
| `edgion-resources/src/resources/link_sys/elasticsearch.rs:214` | `enum EsVendor` | Only referenced inside unit tests in elasticsearch.rs |
| `edgion-resources/src/resources/link_sys/elasticsearch.rs:173` | `enum EsFailedCacheType` | Only referenced inside unit tests in elasticsearch.rs |

---

## Category D — Stale `#[allow(dead_code)]` Annotations (Items Are Alive)

These annotations should be **removed** (the item IS used — annotation is noise).

| File:Line | Item | Evidence |
|-----------|------|---------|
| `edgion-gateway/src/runtime/server/error_response.rs:225` | `end_response_421` | Called in `pg_request_filter.rs:75` |
| `edgion-gateway/src/runtime/store/gateway.rs:41` | `GatewayStore::get_gateway` | Called in `matching/route.rs:116,130` and `pg_request_filter.rs:53` — REMOVED in refactor (not found 2026-06-13) |
| `edgion-common/src/secret/secret_store.rs:65` | `SecretStore::count()` | Called in `api/debug_handlers.rs:37` |

---

## Category E — Intentionally Suppressed (Do Not Remove)

These carry `#[allow(dead_code)]` for legitimate reasons. Not candidates for removal.

| File:Line | Item | Reason |
|-----------|------|--------|
| `edgion-cli/src/cli/client.rs:39` | `EdgionClient::socket_path` | Unix socket transport not yet wired |
| `src/core/center/fed_sync/server/mod.rs:50` | `SseEnvelope::sync_version` | Reserved for stale-response detection — REMOVED in refactor (not found 2026-06-13; EdgionCenter extracted to separate repo) |
| `edgion-resources/src/resource/kind.rs:68` | `_compile_time_sync_check()` | Intentional exhaustive-match guard |
| `edgion-resources/src/resource/macros.rs:177` | `_exhaustive_check()` | Same pattern inside macro — REMOVED in refactor (not found 2026-06-13) |
| Various gateway task-tagged items (7) | task 15/16 items | Planned validator binary — keep |
| `without_conditions()` × 4 in `edgion-gateway/src/plugins/runtime/conditional_filter.rs` | API surface constructors | Intentional public API surface |

---

## Summary Table

| Category | Count | Recommended Action |
|----------|-------|--------------------|
| A — Compiler-annotated / 0 callers | 38 items | **Delete** |
| B — Test-only usage | 16 items | **Delete** (verify tests still pass) |
| C — Schema types for unimplemented features | 12 items | **Delete** (needs deeper design verification) |
| D — Stale `#[allow]` annotations | 3 items | **Remove annotation only** |
| E — Intentional (keep) | 13 items | No action |

**Total actionable candidates: 69 items** (A+B+C+D)
