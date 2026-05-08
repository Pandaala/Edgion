# Opus Deep Review — Confirmed Dead Code Deletion List

Source of truth: `findings-consolidated.md`. Each candidate was independently
verified by reading the file at the cited line and running grep across `src/`,
`examples/`, and `tests/`. Re-export chains were checked. The crate is a lib
(`edgion`) but is not published externally; only the four in-tree binaries and
the in-tree examples consume it.

---

## Safe to Delete: Category A (confirmed)

### A1: Gateway

- [ ] `src/core/gateway/lb/lb_policy/config.rs:25-28` — `pub fn parse_policies()`
  - Verification: only the definition matches. Not in any `pub use`.
  - Delete: lines 24-28 (annotation + signature + body).
  - Risk: Low.

- [ ] `src/core/gateway/lb/lb_policy/config.rs:31-39` — `fn parse_service_key()`
  - Verification: private, only the definition matches. Was likely added together with `parse_policies`.
  - Delete: lines 30-39.
  - Risk: Low.

- [ ] `src/core/gateway/link_sys/runtime/data_sender.rs:44-46` — `pub type StringDataSender`
  - Verification: only the definition matches; not re-exported.
  - Delete: lines 44-46 (doc + annotation + alias). Verify no `pub use` in `mod.rs`.
  - Risk: Low.

- [ ] `src/core/gateway/plugins/http/dsl/lang/parser/mod.rs:85-90` — `pub(super) fn Parser::span_at()`
  - Verification: only definition matches. Debug helper.
  - Delete: lines 85-90.
  - Risk: Low.

- [ ] `src/core/gateway/runtime/server/error_response.rs:147-184` — `pub async fn end_response_403()`
  - Verification: only the definition matches. NOT in the `pub use` re-exports (`gateway/mod.rs:20`, `runtime/mod.rs:15`, `server/mod.rs:9`) which list only 400/404/421/500/502/503.
  - Delete: lines 147-184 (3-line doc + annotation + function).
  - Risk: Low. (403 is now handled via plugins, not via this helper.)

- [ ] `src/core/gateway/backends/health/check/config_store.rs:86-88` — `pub fn endpoint_slice_keys()`
  - Verification: only definition matches. Note: this item is NOT marked `#[allow(dead_code)]` despite the consolidated findings hint — the compiler must currently allow it because `pub` items don't trigger `dead_code` warnings inside a `lib`. Still genuinely unused.
  - Delete: lines 86-88.
  - Risk: Low.

- [ ] `src/core/gateway/backends/health/check/config_store.rs:90-92` — `pub fn endpoint_keys()`
  - Verification: only definition matches.
  - Delete: lines 90-92.
  - Risk: Low.

- [ ] `src/core/gateway/backends/policy/backend_tls/backend_tls_policy_store.rs:281-284` — `pub fn get_all()`
  - Verification: no callers; not re-exported.
  - Delete: lines 281-284 (incl. doc comment).
  - Risk: Low.

- [ ] `src/core/gateway/backends/policy/backend_tls/backend_tls_policy_store.rs:286-296` — `pub fn get_by_namespace()`
  - Verification: no callers; not re-exported.
  - Delete: lines 286-296 (incl. doc comment).
  - Risk: Low.

- [ ] `src/core/gateway/tls/store/tls_store.rs:245-253` — `fn rebuild_matcher()` (private)
  - Verification: only definition matches. Doc says "Prefer `rebuild_matcher_from_data` when the caller already holds a lock" — the alternate `rebuild_matcher_from_data` is the only one actually used.
  - Delete: lines 245-253 (doc + annotation + function).
  - Risk: Low.

### A2: Controller

- [ ] `src/core/controller/conf_sync/conf_server/grpc_server.rs:52-69` — `pub fn with_static_server()`
  - Verification: only definition matches.
  - Delete: lines 52-69 (doc + annotation + function).
  - Risk: Low.

- [ ] `src/core/controller/conf_sync/conf_server/grpc_server.rs:157-~190` — `pub async fn serve_with_reflection()`
  - Verification: only definition matches; alternate entry point not wired into any binary. Production uses `serve_with_shutdown`.
  - Delete: the full function body (verify by reading the closing `}`).
  - Risk: Low.

- [ ] `src/core/controller/conf_sync/conf_server/client_registry.rs:23-27` — `ClientMeta::client_name` and `ClientMeta::connected_at` fields
  - Verification: written at line 44-45 but never read. Removing both fields will turn `ClientMeta` into a unit struct — consider replacing the `HashMap<String, ClientMeta>` with `HashSet<String>` or `HashMap<String, ()>`. The `client_name` parameter passed into `register()` is also unused after field removal — clean it up.
  - Delete: lines 22-27 (struct fields) AND line 38-46 (simplify `register` to drop `client_name` argument; trace log already prints it from caller's scope).
  - Risk: Low-Medium (struct simplification touches caller signature in `config_sync_server.rs`).

- [ ] `src/core/controller/conf_mgr/sync_runtime/resource_processor/handler.rs:142-154` — `pub struct DefaultHandler`
  - Verification: only used inside `#[cfg(test)]` block at `processor.rs:732`.
  - Delete: lines 142-154 (doc + annotation + struct + impl).
  - Action: also remove `processor.rs:735` `use ... DefaultHandler` and update tests to construct an inline mock or remove the tests that depend on it.
  - Risk: Low (test-only).

- [ ] `src/core/controller/conf_mgr/sync_runtime/resource_processor/secret_utils/secret_store.rs:70-74` — `pub fn SecretStore::keys()`
  - Verification: only definition matches.
  - Delete: lines 70-74 (doc + annotation + function).
  - Risk: Low.

- [ ] `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/backend_tls_policy.rs:595-599` — `fn secret_has_ca_cert()` (private)
  - Verification: only definition matches.
  - Delete: lines 595-599 (annotation + inline + function).
  - Risk: Low.

- [ ] `src/core/controller/api/types.rs:42-46` — `pub fn AdminState::is_ready()`
  - Verification: no external callers (the matches in `manager.rs`, `cache.rs`, `processor_registry.rs`, `processor.rs` etc. are different `is_ready` methods on different types).
  - Delete: lines 42-46 (doc + annotation + function).
  - Risk: Low.

### A3: Common (not annotated, but confirmed unused)

- [ ] `src/core/common/utils/net.rs:14-17` — `pub fn parse_optional_listen_addr()`
  - Verification: only definition matches.
  - Delete: lines 14-17.
  - Risk: Low.

- [ ] `src/core/common/utils/net.rs:19-25` — `pub fn normalize_grpc_endpoint()`
  - Verification: only definition matches.
  - Delete: lines 19-25.
  - Risk: Low.

- [ ] `src/core/common/utils/net.rs:27-29` — `pub fn default_operator_addr()`
  - Verification: only definition matches. Note: the constant `DEFAULT_OPERATOR_GRPC_ADDR` (line 7) IS used elsewhere; keep the constant, drop only this getter.
  - Delete: lines 27-29.
  - Risk: Low.

- [ ] `src/core/common/utils/mod.rs:123-~143` — `pub fn format_resource_info()`
  - Verification: only definition matches.
  - Delete: the function body and its doc comment.
  - Risk: Low.

- [ ] `src/core/common/matcher/host_match/radix_match/radix_host_match.rs:51-~55` — `pub fn export_hosts()`
  - Verification: only definition matches.
  - Delete: lines 51-~55 (doc + function).
  - Risk: Low.

### A4: CTL

- [ ] `src/core/ctl/cli/output.rs:92-95` — `pub fn print_message()`
  - Verification: only definition matches.
  - Delete: lines 92-95 (doc + function).
  - Risk: Low.

### A5: Types — Constants

- [ ] `src/types/constants/app.rs:9-10` — `LOG_PREFIX_OPERATOR`
  - Verification: only definition matches; not imported via star-glob in any consumer.
  - Delete: lines 9-10. (`pub use app::*` in `constants/mod.rs:26` will lose this — no breakage since unused.)
  - Risk: Low.

- [ ] `src/types/constants/app.rs:12-13` — `LOG_PREFIX_GATEWAY`
  - Verification: only definition matches.
  - Delete: lines 12-13.
  - Risk: Low.

- [ ] `src/types/constants/app.rs:15-16` — `DEFAULT_LOG_DIR`
  - Verification: only definition matches.
  - Delete: lines 15-16.
  - Risk: Low.

- [ ] `src/types/constants/app.rs:18-19` — `DEFAULT_CONFIG_DIR`
  - Verification: only definition matches.
  - Delete: lines 18-19.
  - Risk: Low.

- [ ] `src/types/constants/app.rs:21-22` — `DEFAULT_WORK_DIR`
  - Verification: only definition matches.
  - Delete: lines 21-22.
  - Risk: Low.

- [ ] `src/types/constants/annotations.rs:62` — `k8s::LAST_APPLIED_CONFIG`
  - Verification: only definition matches.
  - Delete: line 62 (and its doc comment).
  - Risk: Low.

- [ ] `src/types/constants/labels.rs:13` — `edgion::MANAGED_BY`
  - Verification: only definition matches. Note: a duplicate string literal `"edgion.io/managed-by"` exists at `controller/services/acme/secret_io.rs:18` as `EDGION_IO_MANAGED_BY` (private). Removing the unused shared constant does not affect that file. (Bonus refactor: the acme private constant could later be replaced by the shared one once the shared one has callers, but that is out of scope for this dead-code pass.)
  - Delete: line 13 (and its doc comment).
  - Risk: Low.

- [ ] `src/types/constants/headers.rs:1-48` — entire `headers` module (16 constants in three sub-modules)
  - Verification: only definitions match. None of the constants (`X_FORWARDED_FOR`, `X_REAL_IP`, `UPGRADE`, etc.) are imported anywhere; production code uses raw string literals.
  - Delete: the entire file `headers.rs` and the line `pub mod headers;` in `constants/mod.rs:21`. Also drop the doc-example reference at `constants/mod.rs:15`.
  - Risk: Low (purely additive constants module never wired in).

### A6: Types — Enums

- [ ] `src/types/filters.rs:63-70` — `pub enum PluginTags`
  - Verification: only definition matches; never instantiated, matched, or serialized.
  - Delete: lines 63-70 (doc + derive + enum).
  - Risk: Low.

---

## Safe to Delete: Category B (test-only, confirmed)

These are `pub` items that are only invoked from tests in the same module.
Removing them requires also pruning the matching test cases.

- [ ] `src/core/common/utils/net.rs:80-87` — `pub fn is_localhost()`
  - Verification: 8 hits, all in `#[cfg(test)] mod tests` (lines 113-167). Doc says "kept for backward compatibility" but no caller exists.
  - Delete: function + dependent test cases (`test_is_localhost_ipv4`, `_ipv6`, `_unix_socket`).
  - Risk: Low.

- [ ] `src/core/common/utils/mod.rs:74-80` — `pub fn check_need_version()`
  - Verification: only used in tests at `mod.rs:174,183,187,195`.
  - Delete: function + matching tests. Also remove `next_resource_version()` if it becomes unused (verify separately).
  - Risk: Low.

- [ ] `src/core/common/matcher/ip_radix_tree/types.rs:134-136` — `pub fn u32_to_ipv4()`
  - Verification: tests at lines 153, 178; no production usage.
  - Delete: function + test calls.
  - Risk: Low.

- [ ] `src/core/common/matcher/ip_radix_tree/types.rs:138-141` — `pub fn u128_to_ipv6()`
  - Verification: only test at line 165.
  - Delete: function + test calls.
  - Risk: Low.

- [ ] `src/core/common/matcher/ip_radix_tree/matcher.rs:84-92` — `has_ipv4_rules`, `has_ipv6_rules`
  - Verification: both only used in tests at lines 232, 233, 251, 252, 276, 277.
  - Delete: methods + the matcher tests that depend on them.
  - Risk: Low.

- [ ] `src/core/common/matcher/ip_radix_tree/matcher.rs:94-116` — `pub fn IpRadixMatcher::stats()` + `pub struct MatcherStats`
  - Verification: `MatcherStats` is in the `pub use matcher::{..., MatcherStats}` re-export at `mod.rs:111`, but no consumer of the re-export exists in `src/`, `examples/`, or `tests/`. `stats()` itself has no callers outside the file.
  - Delete: function + struct + the `MatcherStats` token from `mod.rs:111` re-export.
  - Risk: Low.

- [ ] `src/core/common/matcher/ip_radix_tree/frozen_v4.rs:159-~167` — `pub fn FrozenIpV4RadixTree::stats()`
  - Verification: only call site is `IpRadixMatcher::stats()` (which itself is dead). When that goes, this becomes orphaned.
  - Delete: function + the `TreeStats` struct local to this file (line 169) IF no other consumers exist (verify after deleting `IpRadixMatcher::stats`).
  - Risk: Low.

- [ ] `src/core/common/matcher/ip_radix_tree/frozen_v6.rs:159-~167` — `pub fn FrozenIpV6RadixTree::stats()`
  - Verification: same — only used by the dead `IpRadixMatcher::stats()`.
  - Delete: function + local `TreeStats`.
  - Risk: Low.

- [ ] `src/core/common/matcher/ip_radix_tree/matcher.rs:167-175` — `ipv4_rule_count` and `ipv6_rule_count` on the builder
  - Verification: only used in tests at lines 218-219.
  - Delete: methods + dependent tests.
  - Risk: Low.

- [ ] `src/core/common/matcher/radix_tree/frozen.rs:548-~565` — `pub fn FrozenRadixTree::match_exact()`
  - Verification: single caller is at `radix_host_match.rs:313`, which is inside `#[cfg(test)]` (test block starts at line 175 of that file).
  - Delete: function + the test that uses it (`test_engine_tree_access` around line 305).
  - Risk: Low.

- [ ] `src/core/common/matcher/radix_tree/frozen.rs:~567-~580` — `pub fn FrozenRadixTree::stats()` + `pub struct TreeStats` (in this file)
  - Verification: never called from production. (Note: `frozen_v4.rs` and `frozen_v6.rs` each define their own `TreeStats` — they are separate types in separate modules.)
  - Delete: function + struct.
  - Risk: Low.

- [ ] `src/core/common/matcher/ip_radix_tree/types.rs:114-130` — `IpCidr::is_v4`, `is_v6`, `prefix_len`
  - Verification: callers always pattern-match on the variant directly (e.g., `match cidr { IpCidr::V4 { .. } => ..., IpCidr::V6 { .. } => ... }` at `matcher.rs:161-163`). No `.is_v4()`/`.is_v6()`/`.prefix_len()` call sites in production.
  - Delete: lines 114-130.
  - Risk: Low.

- [ ] `src/core/controller/conf_mgr/sync_runtime/resource_processor/secret_utils/secret_ref.rs:14-17` — `pub fn new_secret_ref_manager()`
  - Verification: callers are at lines 26, 46, 67 — all in `#[cfg(test)]` (block starts line 19). The wrapper itself is also re-exported at `secret_utils/mod.rs:10`. If preserved as a constructor convention, downgrade to `#[cfg(test)]` instead of deleting.
  - Delete: function + the `new_secret_ref_manager` from the `pub use` line in `mod.rs:10`. Tests should call `SecretRefManager::with_component("...")` directly (one-line replacement).
  - Risk: Low.

### Keep — Not Truly Dead (rejected from B)

- `src/core/controller/conf_mgr/sync_runtime/resource_processor/status_utils.rs:376` — `pub fn accepted_condition_with_message()`
  - Reason: the scanner observation "should be inlined" is a refactor recommendation, not dead code. The function is `pub` and is called by `accepted_condition()` (line 372) which is itself called from at least three production handlers (`processor.rs:880`, `link_sys.rs:181`, `edgion_gateway_config.rs:43`). Inlining would lose the named entry point for any future status-message customization. Leave as-is.

---

## Safe to Delete: Category C (schema types)

### Confirmed

- [ ] `src/types/resources/edgion_tls.rs` — `EdgionTlsExtend` and 5 sub-types
  - Items: `OcspStaplingConfig` (lines 101-119), `EdgionTlsExtend` (170-197), `SessionTicketConfig` (199-215), `SessionCacheConfig` (217-230), `RevocationCheckConfig` + `RevocationMode` (232-253), `EarlyDataConfig` (255-265).
  - Verification: every reference to these types is internal to `edgion_tls.rs` (the field `extend: Option<EdgionTlsExtend>` on `EdgionTlsSpec` at line 159). The only writes of `extend: None` are inside test blocks (`edgion_tls.rs:510, 591` after `#[cfg(test)]` at line 491; `controller/.../handlers/edgion_tls.rs:372` after `#[cfg(test)]` at line 329). No reads anywhere.
  - Delete: all 6 type definitions plus the `extend` field on `EdgionTlsSpec` (line 157-159) plus the test-time `extend: None` initializers (which become unnecessary once the field is gone).
  - Risk: Medium — this is a schema-visible field. If any user-supplied YAML for `EdgionTls` carries `spec.extend: {...}`, removing the field will cause `serde` to reject the manifest unless we keep `#[serde(default, skip_serializing_if = "...")]` and a typed-but-unused alias. Recommended path: (a) drop the types and field, (b) add a release note: "EdgionTls spec.extend is removed; the field was reserved-only and never enforced. If you have it set, delete it from your YAML." If safer, keep the field as `#[serde(default, skip)] pub extend: Option<serde_json::Value>` for one release before full removal — but that defeats the simplification goal.

- [ ] `src/types/resources/grpc_route.rs:356-377` — `pub struct GRPCRouteRetry`
  - **Reclassified**: this is a FALSE POSITIVE. `GRPCRouteRetry` IS read by `parse_retry()` at `grpc_route.rs:492-554` (specifically `retry.attempts`, `retry.backoff`, `retry.codes`). The scanner missed this. Move to "Keep".

- [ ] `src/types/resources/grpc_route.rs:323-334` — `pub struct GRPCRequestMirrorFilter`
  - Verification: referenced as field type on `GRPCRouteFilter.request_mirror` (line 286) and as the `RequestMirror` variant of `GRPCRouteFilterType` (line 301). The plugin runtime's GRPC filter loop at `plugin_runtime.rs:265-294` matches `RequestHeaderModifier`, `ResponseHeaderModifier`, `ExtensionRef` and falls through `_ => {}` for everything else, so the `RequestMirror` variant is silently dropped — no behavior. The whole feature is unimplemented.
  - Delete: `GRPCRequestMirrorFilter` struct (lines 323-334), the `request_mirror` field on `GRPCRouteFilter` (lines 284-286), the `RequestMirror` variant on `GRPCRouteFilterType` (lines 300-301).
  - Risk: Medium — same schema-compat caveat as `EdgionTlsExtend`. If a user has `filters: [{type: RequestMirror, requestMirror: {...}}]` in their `GRPCRoute` YAML, removal turns it into a deserialize error. Document in release notes.

- [ ] `src/types/resources/link_sys/elasticsearch.rs:160-208` — `enum EsFailedCacheType`, `enum EsVendor` (and arguably the wrapping `EsFailedCache` / `EsCompatibility`)
  - Verification: `EsFailedCacheType` is the `cache_type` field of `EsFailedCache`; `EsVendor` is the `vendor` field of `EsCompatibility`. Neither wrapping struct is consumed anywhere in `core/gateway/` or `core/controller/`. The `failed_cache` / `compatibility` fields on `ElasticsearchClientConfig` (lines 46, 50) are written/cleared in tests but never read by runtime code.
  - Delete: as a group — `EsFailedCache`, `EsFailedCacheType`, `EsFailedCacheLocalFile`, `EsFailedCacheRedis`, `EsCompatibility`, `EsVendor`, AND the `failed_cache` and `compatibility` fields on `ElasticsearchClientConfig`.
  - Risk: Medium — same schema-compat caveat. If users have `failedCache:` or `compatibility:` blocks in `LinkSys` Elasticsearch configs, removal is a breaking deserialize change. Document.

### Keep — False Positives (rejected from C)

- `src/types/resources/grpc_route.rs:360` — `GRPCRouteRetry`
  - Reason: actively read by `GRPCRoute::parse_retry()` at `grpc_route.rs:498-540` to populate `ParsedRouteTimeouts`. Scanner missed the reflexive in-file caller.

- `src/types/resources/link_sys/webhook.rs:267-` — `ActiveHealthCheck`
  - Reason: actively read at `core/gateway/link_sys/providers/webhook/health.rs:15, 98, 119` and `manager.rs:111` via `runtime.config.health_check.as_ref().and_then(|hc| hc.active.as_ref())`. Scanner confused this `ActiveHealthCheck` (in `link_sys/webhook.rs`) with the structurally similar `ActiveHealthCheckConfig` in `health_check.rs`. Both are alive and distinct types.

- `src/types/resources/gateway_class.rs:62` — `ParametersReference`
  - Reason: actively read at `core/controller/.../handlers/gateway_class.rs:37` (`gc.spec.parameters_ref`) and `core/gateway/runtime/server/base.rs:111` (`gateway_class.spec.parameters_ref.as_ref()`). The type definition is reachable via the field; both producers and consumers exist.

---

## Remove Annotation Only: Category D

- [ ] `src/core/gateway/runtime/server/error_response.rs:189` — remove `#[allow(dead_code)]` from `end_response_421`
  - Evidence: called at `routes/http/proxy_http/pg_request_filter.rs:75`; re-exported in three `mod.rs` files.

- [ ] `src/core/controller/conf_mgr/sync_runtime/resource_processor/secret_utils/secret_store.rs:65` — remove `#[allow(dead_code)]` from `count()`
  - Evidence: called at `core/controller/api/debug_handlers.rs:37`.

### Reclassified out of D

- `src/core/gateway/runtime/store/gateway.rs:41` — `GatewayStore::get_gateway` is **NOT** alive.
  - Evidence: scanner thought callers `route.rs:116`, `route.rs:130`, `pg_request_filter.rs:53` invoked this method, but those sites all do `if let Some(gw) = index.get_gateway(...)` against a `GatewayInfoIndex`, which has its own `get_gateway` returning `Option<&GatewayListeners>` (`store/listener_gateway_info.rs:58`). `GatewayStore::get_gateway` returns `Result<&Gateway>` and has zero callers.
  - **Move to Category A:** delete the function (lines 40-46) including its annotation. Risk: Low.

---

## Keep (false positives from scanner)

- `src/types/constants/app.rs:4` — `CONTROLLER_NAME`
  - Reason: same module re-exports it as `pub use CONTROLLER_NAME as COMPONENT_EDGION_CONTROLLER` (line 28). The alias `COMPONENT_EDGION_CONTROLLER` is imported at `core/controller/cli/mod.rs:8` and used 25+ times. Removing `CONTROLLER_NAME` would either break the alias or require renaming the source-of-truth identifier. Best left as-is.

- `src/types/resources/grpc_route.rs:360` — `GRPCRouteRetry` (already noted above; alive via `parse_retry`).

- `src/types/resources/link_sys/webhook.rs:270` — `ActiveHealthCheck` (already noted above; alive via webhook health driver).

- `src/types/resources/gateway_class.rs:62` — `ParametersReference` (already noted above; alive via gateway_class handler and gateway base).

- `src/core/controller/conf_mgr/sync_runtime/resource_processor/status_utils.rs:376` — `accepted_condition_with_message` (Cat B): keep as named API surface; not strictly dead.

---

## Summary

| Category | Confirmed | False positives | Notes |
|----------|-----------|-----------------|-------|
| A — Compiler-annotated / 0 callers | 38 (incl. `GatewayStore::get_gateway` reclassified from D) | 1 (`CONTROLLER_NAME`) | All Low risk. |
| B — Test-only | 15 | 1 (`accepted_condition_with_message`) | All Low risk; tests must be pruned in lockstep. |
| C — Schema clusters | 3 clusters (EdgionTlsExtend group; GRPCRequestMirrorFilter; ES enum group) | 3 (`GRPCRouteRetry`, `ActiveHealthCheck`, `ParametersReference`) | Medium risk: each cluster removal is a breaking schema change; release note required. |
| D — Annotation-only | 2 (`end_response_421`, `SecretStore::count`) | 1 (`GatewayStore::get_gateway`, moved to A) | Trivial. |

**Total deletions confirmed:** 38 (A) + 15 (B) + ~15 schema items in 3 clusters (C) + 2 annotation cleanups (D).

**Total false positives caught at this layer:** 6.

### Suggested rollout order

1. Apply all of Category A and Category D first — purely internal, zero schema impact.
2. Apply Category B together with the matching test deletions; run `cargo test --all` to confirm.
3. Apply Category C in a separate PR with a CHANGELOG entry calling out the removed YAML keys (`spec.extend` on `EdgionTls`, `RequestMirror` filter on `GRPCRoute`, `failedCache` / `compatibility` on `LinkSys` Elasticsearch). If any of these schemas have shipped to users, gate behind a release boundary.
