# Opus Deep Review — Confirmed Dead Code Deletion List

Source of truth: `findings-consolidated.md`. Each candidate was independently
verified by reading the file at the cited line and running grep across `src/`,
`examples/`, and `tests/`. Re-export chains were checked. The crate is a lib
(`edgion`) but is not published externally; only the four in-tree binaries and
the in-tree examples consume it.

---

## Safe to Delete: Category A (confirmed)

### A1: Gateway

- [ ] `edgion-gateway/src/lb/lb_policy/config.rs` — `pub fn parse_policies()` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only the definition matches. Not in any `pub use`.
  - Delete: lines 24-28 (annotation + signature + body).
  - Risk: Low.

- [ ] `edgion-gateway/src/lb/lb_policy/config.rs` — `fn parse_service_key()` — REMOVED in refactor (not found 2026-06-13)
  - Verification: private, only the definition matches. Was likely added together with `parse_policies`.
  - Delete: lines 30-39.
  - Risk: Low.

- [ ] `edgion-gateway/src/link_sys/runtime/data_sender.rs` — `pub type StringDataSender` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only the definition matches; not re-exported.
  - Delete: lines 44-46 (doc + annotation + alias). Verify no `pub use` in `mod.rs`.
  - Risk: Low.

- [ ] `edgion-gateway/src/plugins/http/dsl/lang/parser/mod.rs` — `pub(super) fn Parser::span_at()` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only definition matches. Debug helper.
  - Delete: lines 85-90.
  - Risk: Low.

- [ ] `edgion-gateway/src/runtime/server/error_response.rs` — `pub async fn end_response_403()` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only the definition matches. NOT in the `pub use` re-exports (`gateway/mod.rs:20`, `runtime/mod.rs:15`, `server/mod.rs:9`) which list only 400/404/421/500/502/503.
  - Delete: lines 147-184 (3-line doc + annotation + function).
  - Risk: Low. (403 is now handled via plugins, not via this helper.)

- [ ] `edgion-gateway/src/backends/health/check/config_store.rs` — `pub fn endpoint_slice_keys()` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only definition matches. Note: this item is NOT marked `#[allow(dead_code)]` despite the consolidated findings hint — the compiler must currently allow it because `pub` items don't trigger `dead_code` warnings inside a `lib`. Still genuinely unused.
  - Delete: lines 86-88.
  - Risk: Low.

- [ ] `edgion-gateway/src/backends/health/check/config_store.rs` — `pub fn endpoint_keys()` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only definition matches.
  - Delete: lines 90-92.
  - Risk: Low.

- [ ] `edgion-gateway/src/backends/policy/backend_tls/backend_tls_policy_store.rs` — `pub fn get_all()` — REMOVED in refactor (not found 2026-06-13)
  - Verification: no callers; not re-exported.
  - Delete: lines 281-284 (incl. doc comment).
  - Risk: Low.

- [ ] `edgion-gateway/src/backends/policy/backend_tls/backend_tls_policy_store.rs` — `pub fn get_by_namespace()` — REMOVED in refactor (not found 2026-06-13)
  - Verification: no callers; not re-exported.
  - Delete: lines 286-296 (incl. doc comment).
  - Risk: Low.

- [ ] `edgion-gateway/src/tls/store/tls_store.rs` — `fn rebuild_matcher()` (private) — REMOVED in refactor (not found 2026-06-13)
  - Verification: only definition matches. Doc says "Prefer `rebuild_matcher_from_data` when the caller already holds a lock" — the alternate `rebuild_matcher_from_data` is the only one actually used.
  - Delete: lines 245-253 (doc + annotation + function).
  - Risk: Low.

### A2: Controller

- [ ] `edgion-controller/src/conf_sync/conf_server/grpc_server.rs` — `pub fn with_static_server()` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only definition matches.
  - Delete: lines 52-69 (doc + annotation + function).
  - Risk: Low.

- [ ] `edgion-controller/src/conf_sync/conf_server/grpc_server.rs` — `pub async fn serve_with_reflection()` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only definition matches; alternate entry point not wired into any binary. Production uses `serve_with_shutdown`.
  - Delete: the full function body (verify by reading the closing `}`).
  - Risk: Low.

- [ ] `edgion-controller/src/conf_sync/conf_server/client_registry.rs` — `ClientMeta::client_name` and `ClientMeta::connected_at` fields — REMOVED in refactor (not found 2026-06-13)
  - Verification: written at line 44-45 but never read. Removing both fields will turn `ClientMeta` into a unit struct — consider replacing the `HashMap<String, ClientMeta>` with `HashSet<String>` or `HashMap<String, ()>`. The `client_name` parameter passed into `register()` is also unused after field removal — clean it up.
  - Delete: lines 22-27 (struct fields) AND line 38-46 (simplify `register` to drop `client_name` argument; trace log already prints it from caller's scope).
  - Risk: Low-Medium (struct simplification touches caller signature in `config_sync_server.rs`).

- [ ] `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handler.rs` — `pub struct DefaultHandler` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only used inside `#[cfg(test)]` block at `processor.rs:732`.
  - Delete: lines 142-154 (doc + annotation + struct + impl).
  - Action: also remove `processor.rs:735` `use ... DefaultHandler` and update tests to construct an inline mock or remove the tests that depend on it.
  - Risk: Low (test-only).

- [ ] `edgion-common/src/secret/secret_store.rs` — `pub fn SecretStore::keys()` — REMOVED in refactor (not found 2026-06-13)
  - Note: `SecretStore` moved from `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/secret_utils/secret_store.rs` to `edgion-common/src/secret/secret_store.rs` in the refactor.
  - Verification: only definition matches.
  - Delete: lines 70-74 (doc + annotation + function).
  - Risk: Low.

- [ ] `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/backend_tls_policy.rs` — `fn secret_has_ca_cert()` (private) — REMOVED in refactor (not found 2026-06-13)
  - Verification: only definition matches.
  - Delete: lines 595-599 (annotation + inline + function).
  - Risk: Low.

- [ ] `edgion-controller/src/api/types.rs:47-48` — `pub fn AdminState::is_ready()`
  - Verification: no external callers (the matches in `manager.rs`, `cache.rs`, `processor_registry.rs`, `processor.rs` etc. are different `is_ready` methods on different types).
  - Delete: lines 47-48 (doc + annotation + function).
  - Risk: Low.

### A3: Common (not annotated, but confirmed unused)

- [ ] `edgion-common/src/utils/net.rs` — `pub fn parse_optional_listen_addr()` — REMOVED in refactor (not found 2026-06-13)
  - Note: `src/core/common/utils/net.rs` content was split; `is_localhost` moved to `edgion-gateway/src/util/net.rs`; the three gateway-util functions are not present in any crate.
  - Verification: only definition matches.
  - Delete: lines 14-17.
  - Risk: Low.

- [ ] `edgion-common/src/utils/net.rs` — `pub fn normalize_grpc_endpoint()` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only definition matches.
  - Delete: lines 19-25.
  - Risk: Low.

- [ ] `edgion-common/src/utils/net.rs` — `pub fn default_operator_addr()` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only definition matches. Note: the constant `DEFAULT_OPERATOR_GRPC_ADDR` (line 7) IS used elsewhere; keep the constant, drop only this getter.
  - Delete: lines 27-29.
  - Risk: Low.

- [ ] `edgion-common/src/utils/mod.rs` — `pub fn format_resource_info()` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only definition matches.
  - Delete: the function body and its doc comment.
  - Risk: Low.

- [ ] `edgion-resources/src/matcher/host_match/radix_match/radix_host_match.rs` — `pub fn export_hosts()` — REMOVED in refactor (not found 2026-06-13)
  - Note: matcher subtree moved from `src/core/common/matcher/` to `edgion-resources/src/matcher/`.
  - Verification: only definition matches.
  - Delete: lines 51-~55 (doc + function).
  - Risk: Low.

### A4: CTL

- [ ] `edgion-cli/src/cli/output.rs` — `pub fn print_message()` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only definition matches.
  - Delete: lines 92-95 (doc + function).
  - Risk: Low.

### A5: Types — Constants

- [ ] `edgion-resources/src/constants/app.rs` — `LOG_PREFIX_OPERATOR` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only definition matches; not imported via star-glob in any consumer.
  - Delete: lines 9-10. (`pub use app::*` in `constants/mod.rs:26` will lose this — no breakage since unused.)
  - Risk: Low.

- [ ] `edgion-resources/src/constants/app.rs` — `LOG_PREFIX_GATEWAY` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only definition matches.
  - Delete: lines 12-13.
  - Risk: Low.

- [ ] `edgion-resources/src/constants/app.rs` — `DEFAULT_LOG_DIR` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only definition matches.
  - Delete: lines 15-16.
  - Risk: Low.

- [ ] `edgion-resources/src/constants/app.rs` — `DEFAULT_CONFIG_DIR` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only definition matches.
  - Delete: lines 18-19.
  - Risk: Low.

- [ ] `edgion-resources/src/constants/app.rs` — `DEFAULT_WORK_DIR` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only definition matches.
  - Delete: lines 21-22.
  - Risk: Low.

- [ ] `edgion-resources/src/constants/annotations.rs` — `k8s::LAST_APPLIED_CONFIG` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only definition matches.
  - Delete: line 62 (and its doc comment).
  - Risk: Low.

- [ ] `edgion-resources/src/constants/labels.rs` — `edgion::MANAGED_BY` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only definition matches. Note: a duplicate string literal `"edgion.io/managed-by"` exists at `controller/services/acme/secret_io.rs:18` as `EDGION_IO_MANAGED_BY` (private). Removing the unused shared constant does not affect that file. (Bonus refactor: the acme private constant could later be replaced by the shared one once the shared one has callers, but that is out of scope for this dead-code pass.)
  - Delete: line 13 (and its doc comment).
  - Risk: Low.

- [ ] `edgion-resources/src/constants/headers.rs` — entire `headers` module (16 constants in three sub-modules) — REMOVED in refactor (file not found 2026-06-13)
  - Verification: only definitions match. None of the constants (`X_FORWARDED_FOR`, `X_REAL_IP`, `UPGRADE`, etc.) are imported anywhere; production code uses raw string literals.
  - Delete: the entire file `headers.rs` and the line `pub mod headers;` in `constants/mod.rs:21`. Also drop the doc-example reference at `constants/mod.rs:15`.
  - Risk: Low (purely additive constants module never wired in).

### A6: Types — Enums

- [ ] `edgion-resources/src/filters.rs` — `pub enum PluginTags` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only definition matches; never instantiated, matched, or serialized.
  - Delete: lines 63-70 (doc + derive + enum).
  - Risk: Low.

---

## Safe to Delete: Category B (test-only, confirmed)

These are `pub` items that are only invoked from tests in the same module.
Removing them requires also pruning the matching test cases.

- [ ] `edgion-gateway/src/util/net.rs:74` — `pub fn is_localhost()`
  - Note: moved from `src/core/common/utils/net.rs:80-87` to `edgion-gateway/src/util/net.rs:74` in the refactor.
  - Verification: 8 hits, all in `#[cfg(test)] mod tests` (lines 166-213). Doc says "kept for backward compatibility" but no caller exists.
  - Delete: function + dependent test cases (`test_is_localhost_ipv4`, `_ipv6`, `_unix_socket`).
  - Risk: Low.

- [ ] `edgion-common/src/utils/mod.rs` — `pub fn check_need_version()` — REMOVED in refactor (not found 2026-06-13)
  - Verification: only used in tests at `mod.rs:174,183,187,195`.
  - Delete: function + matching tests. Also remove `next_resource_version()` if it becomes unused (verify separately).
  - Risk: Low.

- [ ] `edgion-resources/src/matcher/ip_radix_tree/types.rs:134` — `pub fn u32_to_ipv4()`
  - Note: moved from `src/core/common/matcher/ip_radix_tree/types.rs:134-136`; matcher subtree is now in `edgion-resources`.
  - Verification: tests at lines 153, 178; no production usage.
  - Delete: function + test calls.
  - Risk: Low.

- [ ] `edgion-resources/src/matcher/ip_radix_tree/types.rs:139-141` — `pub fn u128_to_ipv6()`
  - Note: moved from `src/core/common/matcher/ip_radix_tree/types.rs:138-141`.
  - Verification: only test at line 165.
  - Delete: function + test calls.
  - Risk: Low.

- [ ] `edgion-resources/src/matcher/ip_radix_tree/matcher.rs:91-96` — `has_ipv4_rules`, `has_ipv6_rules`
  - Note: moved from `src/core/common/matcher/ip_radix_tree/matcher.rs:84-92`.
  - Verification: both only used in tests at lines 278, 279, 297, 298, 322, 323.
  - Delete: methods + the matcher tests that depend on them.
  - Risk: Low.

- [ ] `edgion-resources/src/matcher/ip_radix_tree/matcher.rs:101-113` — `pub fn IpRadixMatcher::stats()` + `pub struct MatcherStats`
  - Note: moved from `src/core/common/matcher/ip_radix_tree/matcher.rs:94-116`.
  - Verification: `MatcherStats` is in the `pub use matcher::{..., MatcherStats}` re-export at `mod.rs:116`, but no consumer of the re-export exists in `src/`, `examples/`, or `tests/`. `stats()` itself has no callers outside the file.
  - Delete: function + struct + the `MatcherStats` token from `mod.rs:116` re-export.
  - Risk: Low.

- [ ] `edgion-resources/src/matcher/ip_radix_tree/frozen_v4.rs:142` — `pub fn FrozenIpV4RadixTree::stats()`
  - Note: moved from `src/core/common/matcher/ip_radix_tree/frozen_v4.rs:159-~167`.
  - Verification: only call site is `IpRadixMatcher::stats()` (which itself is dead). When that goes, this becomes orphaned.
  - Delete: function + the `TreeStats` struct local to this file (line 152) IF no other consumers exist (verify after deleting `IpRadixMatcher::stats`).
  - Risk: Low.

- [ ] `edgion-resources/src/matcher/ip_radix_tree/frozen_v6.rs:142` — `pub fn FrozenIpV6RadixTree::stats()`
  - Note: moved from `src/core/common/matcher/ip_radix_tree/frozen_v6.rs:159-~167`.
  - Verification: same — only used by the dead `IpRadixMatcher::stats()`.
  - Delete: function + local `TreeStats` (line 152).
  - Risk: Low.

- [ ] `edgion-resources/src/matcher/ip_radix_tree/matcher.rs:212-217` — `ipv4_rule_count` and `ipv6_rule_count` on the builder
  - Note: moved from `src/core/common/matcher/ip_radix_tree/matcher.rs:167-175`.
  - Verification: only used in tests at lines 264-265.
  - Delete: methods + dependent tests.
  - Risk: Low.

- [ ] `edgion-resources/src/matcher/radix_tree/frozen.rs:548-565` — `pub fn FrozenRadixTree::match_exact()`
  - Note: moved from `src/core/common/matcher/radix_tree/frozen.rs:548-~565`; line range unchanged.
  - Verification: single caller is at `radix_host_match.rs:307`, which is inside `#[cfg(test)]` (test block starts at line 169 of that file).
  - Delete: function + the test that uses it (`test_engine_tree_access` around line 305).
  - Risk: Low.

- [ ] `edgion-resources/src/matcher/radix_tree/frozen.rs:567-582` — `pub fn FrozenRadixTree::stats()` + `pub struct TreeStats` (in this file)
  - Note: moved from `src/core/common/matcher/radix_tree/frozen.rs:~567-~580`; line numbers approximately unchanged.
  - Verification: never called from production. (Note: `frozen_v4.rs` and `frozen_v6.rs` each define their own `TreeStats` — they are separate types in separate modules.)
  - Delete: function + struct.
  - Risk: Low.

- [ ] `edgion-resources/src/matcher/ip_radix_tree/types.rs:115-130` — `IpCidr::is_v4`, `is_v6`, `prefix_len`
  - Note: moved from `src/core/common/matcher/ip_radix_tree/types.rs:114-130`.
  - Verification: callers always pattern-match on the variant directly (e.g., `match cidr { IpCidr::V4 { .. } => ..., IpCidr::V6 { .. } => ... }` at `matcher.rs:163-167`). No `.is_v4()`/`.is_v6()`/`.prefix_len()` call sites in production.
  - Delete: lines 115-130.
  - Risk: Low.

- [ ] `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/secret_utils/secret_ref.rs:15` — `pub fn new_secret_ref_manager()`
  - Note: previously at `src/core/controller/conf_mgr/sync_runtime/resource_processor/secret_utils/secret_ref.rs:14-17`.
  - Verification: callers are at lines 26, 46, 67 — all in `#[cfg(test)]` (block starts line 19). The wrapper itself is also re-exported at `secret_utils/mod.rs:9`. If preserved as a constructor convention, downgrade to `#[cfg(test)]` instead of deleting.
  - Delete: function + the `new_secret_ref_manager` from the `pub use` line in `mod.rs:9`. Tests should call `SecretRefManager::with_component("...")` directly (one-line replacement).
  - Risk: Low.

### Keep — Not Truly Dead (rejected from B)

- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/status_utils.rs:404` — `pub fn accepted_condition_with_message()`
  - Reason: the scanner observation "should be inlined" is a refactor recommendation, not dead code. The function is `pub` and is called by `accepted_condition()` (line 399) which is itself called from at least three production handlers (`processor.rs:880`, `link_sys.rs:181`, `edgion_gateway_config.rs:43`). Inlining would lose the named entry point for any future status-message customization. Leave as-is.

---

## Safe to Delete: Category C (schema types)

### Confirmed

- [ ] `edgion-resources/src/resources/edgion_tls.rs` — `EdgionTlsExtend` and 5 sub-types
  - Items: `OcspStaplingConfig` (lines 115+), `EdgionTlsExtend` (192+), `SessionTicketConfig` (221+), `SessionCacheConfig` (239+), `RevocationCheckConfig` + `RevocationMode` (262, 274+), `EarlyDataConfig` (287+).
  - Verification: every reference to these types is internal to `edgion_tls.rs` (the field `extend: Option<EdgionTlsExtend>` on `EdgionTlsSpec` at line 171). The only writes of `extend: None` are inside test blocks (`edgion_tls.rs:502` after `#[cfg(test)]` at line 521; `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_tls.rs:502` after `#[cfg(test)]` at line 401). No reads anywhere.
  - Delete: all 6 type definitions plus the `extend` field on `EdgionTlsSpec` (line 171) plus the test-time `extend: None` initializers (which become unnecessary once the field is gone).
  - Risk: Medium — this is a schema-visible field. If any user-supplied YAML for `EdgionTls` carries `spec.extend: {...}`, removing the field will cause `serde` to reject the manifest unless we keep `#[serde(default, skip_serializing_if = "...")]` and a typed-but-unused alias. Recommended path: (a) drop the types and field, (b) add a release note: "EdgionTls spec.extend is removed; the field was reserved-only and never enforced. If you have it set, delete it from your YAML." If safer, keep the field as `#[serde(default, skip)] pub extend: Option<serde_json::Value>` for one release before full removal — but that defeats the simplification goal.

- [ ] `edgion-resources/src/resources/grpc_route.rs:339` — `pub struct GRPCRouteRetry`
  - **Reclassified**: this is a FALSE POSITIVE. `GRPCRouteRetry` IS read by `parse_retry()` at `grpc_route.rs:472+` (specifically `retry.attempts`, `retry.backoff`, `retry.codes`). The scanner missed this. Move to "Keep".

- [ ] `edgion-resources/src/resources/grpc_route.rs:305` — `pub struct GRPCRequestMirrorFilter`
  - Verification: referenced as field type on `GRPCRouteFilter.request_mirror` (line 265) and as the `RequestMirror` variant of `GRPCRouteFilterType` (line 280). The plugin runtime's GRPC filter loop at `edgion-gateway/src/plugins/runtime/plugin_runtime.rs:331-370` matches `RequestHeaderModifier`, `ResponseHeaderModifier`, `ExtensionRef` and falls through `_ => {}` for everything else, so the `RequestMirror` variant is silently dropped — no behavior. The whole feature is unimplemented.
  - Delete: `GRPCRequestMirrorFilter` struct (line 305+), the `request_mirror` field on `GRPCRouteFilter` (line 265), the `RequestMirror` variant on `GRPCRouteFilterType` (line 280).
  - Risk: Medium — same schema-compat caveat as `EdgionTlsExtend`. If a user has `filters: [{type: RequestMirror, requestMirror: {...}}]` in their `GRPCRoute` YAML, removal turns it into a deserialize error. Document in release notes.

- [ ] `edgion-resources/src/resources/link_sys/elasticsearch.rs:156-214` — `enum EsFailedCacheType`, `enum EsVendor` (and arguably the wrapping `EsFailedCache` / `EsCompatibility`)
  - Verification: `EsFailedCacheType` is the `cache_type` field of `EsFailedCache`; `EsVendor` is the `vendor` field of `EsCompatibility`. Neither wrapping struct is consumed anywhere in `edgion-gateway/` or `edgion-controller/`. The `failed_cache` / `compatibility` fields on `ElasticsearchClientConfig` (lines 46, 50) are written/cleared in tests but never read by runtime code.
  - Delete: as a group — `EsFailedCache`, `EsFailedCacheType`, `EsFailedCacheLocalFile`, `EsFailedCacheRedis`, `EsCompatibility`, `EsVendor`, AND the `failed_cache` and `compatibility` fields on `ElasticsearchClientConfig`.
  - Risk: Medium — same schema-compat caveat. If users have `failedCache:` or `compatibility:` blocks in `LinkSys` Elasticsearch configs, removal is a breaking deserialize change. Document.

### Keep — False Positives (rejected from C)

- `edgion-resources/src/resources/grpc_route.rs:339` — `GRPCRouteRetry`
  - Reason: actively read by `GRPCRoute::parse_retry()` at `grpc_route.rs:472+` to populate `ParsedRouteTimeouts`. Scanner missed the reflexive in-file caller.

- `edgion-resources/src/resources/link_sys/webhook.rs:319` — `ActiveHealthCheck`
  - Reason: actively read at `edgion-gateway/src/link_sys/providers/webhook/health.rs:15, 98, 119` and `manager.rs:111` via `runtime.config.health_check.as_ref().and_then(|hc| hc.active.as_ref())`. Scanner confused this `ActiveHealthCheck` (in `link_sys/webhook.rs`) with the structurally similar `ActiveHealthCheckConfig` in `health_check.rs`. Both are alive and distinct types.

- `edgion-resources/src/resources/gateway_class.rs:62` — `ParametersReference`
  - Reason: actively read at `edgion-controller/src/.../handlers/gateway_class.rs:37` (`gc.spec.parameters_ref`) and `edgion-gateway/src/runtime/server/base.rs:111` (`gateway_class.spec.parameters_ref.as_ref()`). The type definition is reachable via the field; both producers and consumers exist.

---

## Remove Annotation Only: Category D

- [ ] `edgion-gateway/src/runtime/server/error_response.rs:225` — remove `#[allow(dead_code)]` from `end_response_421`
  - Evidence: called at `routes/http/proxy_http/pg_request_filter.rs:75`; re-exported in three `mod.rs` files.
  - Note: `#[allow(dead_code)]` annotation already removed in the refactor (not found 2026-06-13). This item is already done.

- [ ] `edgion-common/src/secret/secret_store.rs:65` — remove `#[allow(dead_code)]` from `count()`
  - Note: `SecretStore` moved from the controller `secret_utils/` path to `edgion-common/src/secret/secret_store.rs` in the refactor.
  - Evidence: called at `edgion-controller/src/api/debug_handlers.rs:56`.
  - Note: `#[allow(dead_code)]` annotation already removed in the refactor (not found 2026-06-13). This item is already done.

### Reclassified out of D

- `edgion-gateway/src/runtime/store/gateway.rs` — `GatewayStore::get_gateway` — REMOVED in refactor (not found 2026-06-13)
  - Evidence: scanner thought callers `route.rs:116`, `route.rs:130`, `pg_request_filter.rs:53` invoked this method, but those sites all do `if let Some(gw) = index.get_gateway(...)` against a `GatewayInfoIndex`, which has its own `get_gateway` returning `Option<&GatewayListeners>` (`store/listener_gateway_info.rs:58`). `GatewayStore::get_gateway` returns `Result<&Gateway>` and has zero callers.
  - **Move to Category A:** delete the function (lines 40-46) including its annotation. Risk: Low.

---

## Keep (false positives from scanner)

- `edgion-resources/src/constants/app.rs:4` — `CONTROLLER_NAME`
  - Reason: same module re-exports it as `pub use CONTROLLER_NAME as COMPONENT_EDGION_CONTROLLER` (line 13). The alias `COMPONENT_EDGION_CONTROLLER` is imported at `edgion-cli/src/cli/mod.rs:8` and used 25+ times. Removing `CONTROLLER_NAME` would either break the alias or require renaming the source-of-truth identifier. Best left as-is.

- `edgion-resources/src/resources/grpc_route.rs:339` — `GRPCRouteRetry` (already noted above; alive via `parse_retry`).

- `edgion-resources/src/resources/link_sys/webhook.rs:319` — `ActiveHealthCheck` (already noted above; alive via webhook health driver).

- `edgion-resources/src/resources/gateway_class.rs:62` — `ParametersReference` (already noted above; alive via gateway_class handler and gateway base).

- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/status_utils.rs:404` — `accepted_condition_with_message` (Cat B): keep as named API surface; not strictly dead.

---

## Summary

| Category | Confirmed | False positives | Notes |
|----------|-----------|-----------------|-------|
| A — Compiler-annotated / 0 callers | 38 (incl. `GatewayStore::get_gateway` reclassified from D) | 1 (`CONTROLLER_NAME`) | All Low risk. Most items already deleted in refactor. |
| B — Test-only | 15 | 1 (`accepted_condition_with_message`) | All Low risk; tests must be pruned in lockstep. |
| C — Schema clusters | 3 clusters (EdgionTlsExtend group; GRPCRequestMirrorFilter; ES enum group) | 3 (`GRPCRouteRetry`, `ActiveHealthCheck`, `ParametersReference`) | Medium risk: each cluster removal is a breaking schema change; release note required. |
| D — Annotation-only | 2 (`end_response_421`, `SecretStore::count`) | 1 (`GatewayStore::get_gateway`, moved to A) | Both annotations already removed in refactor. |

**Total deletions confirmed:** 38 (A) + 15 (B) + ~15 schema items in 3 clusters (C) + 2 annotation cleanups (D).

**Total false positives caught at this layer:** 6.

### Suggested rollout order

1. Apply all of Category A and Category D first — purely internal, zero schema impact.
2. Apply Category B together with the matching test deletions; run `cargo test --all` to confirm.
3. Apply Category C in a separate PR with a CHANGELOG entry calling out the removed YAML keys (`spec.extend` on `EdgionTls`, `RequestMirror` filter on `GRPCRoute`, `failedCache` / `compatibility` on `LinkSys` Elasticsearch). If any of these schemas have shipped to users, gate behind a release boundary.
