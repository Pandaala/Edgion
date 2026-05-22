# Dead Code Candidates in src/core/gateway/

Scanned 384 `.rs` files under `src/core/gateway/`.

---

## #[allow(dead_code)] Suppressions

### Confirmed dead (no callers anywhere)

| File | Line | Item | Notes |
|------|------|------|-------|
| `src/core/gateway/lb/lb_policy/config.rs` | 26 | `parse_policies(s)` | Parses comma-separated policy strings; no callers found in entire `src/` |
| `src/core/gateway/lb/lb_policy/config.rs` | 32 | `parse_service_key(service_key)` | Private fn; only defined, never called |
| `src/core/gateway/link_sys/runtime/data_sender.rs` | 46 | `type StringDataSender` | Type alias for `dyn DataSender<String>`; only defined here, referenced nowhere else |
| `src/core/gateway/plugins/http/dsl/lang/parser/mod.rs` | 85 | `Parser::span_at(&self, input)` | Helper returning a `Span` at a parser offset; no callers |
| `src/core/gateway/runtime/server/error_response.rs` | 150 | `end_response_403(...)` | 403 response helper; no calls in `src/` (403 is currently handled differently in plugins) |

### Suppressions that are legitimate (field/function IS used)

| File | Line | Item | Why suppressed |
|------|------|------|----------------|
| `src/core/gateway/plugins/http/openid_connect/plugin.rs` | 166 | `OpenidConnect::plugin_namespace` | Field used in `session_cookie.rs` impl block; Rust lint sees it as dead in the struct definition file because the read is in a sibling `impl` file. `#[allow]` is correct but could be removed by inlining or reorganising. |
| `src/core/gateway/plugins/http/dsl/lang/compiler/mod.rs` | 33 | `Local::depth` | Field read in `emit.rs` (`local.depth < self.scope_depth`); same cross-file false-positive. |
| `src/core/gateway/plugins/http/openid_connect/plugin.rs` | 131/134/139 | `id_token`, `refresh_token`, `token_type` in `TokenEndpointResponse` | Serde-deserialization fields; lint fires because they are never explicitly read after deserialization. Suppression is correct. |
| `src/core/gateway/runtime/server/error_response.rs` | 189 | `end_response_421(...)` | Used in `pg_request_filter.rs:75`. The `#[allow]` is stale — can be removed. |
| `src/core/gateway/plugins/runtime/conditional_filter.rs` | 43/89/143/189 | `without_conditions()` on all four conditional wrapper types | Convenience constructors; not called today but are API-surface helpers. Suppression intentional. |
| `src/core/gateway/plugins/runtime/conditional_filter.rs` | 244 | `MockRequestFilter::run_count()` | Test-only helper inside `#[cfg(test)]` block. Suppression correct. |
| `src/core/gateway/cache/lru.rs` | 456 | `TtlScenario::name` | Test struct field for scenario labelling; never read programmatically. Suppression correct. |
| `src/core/gateway/tls/store/conf_handler.rs` | 40 | `create_test_tls()` | Inside `#[cfg(test)]`. Suppression correct. |
| `src/core/gateway/tls/store/tls_store.rs` | 249 | `TlsStore::rebuild_matcher()` | Wrapper around `rebuild_matcher_from_data`; exists as a convenience API not yet called. |
| `src/core/gateway/runtime/store/gateway.rs` | 41 | `GatewayStore::get_gateway()` | Used in `runtime/matching/route.rs:116,130` and `pg_request_filter.rs:53`. `#[allow]` is stale — can be removed. |
| `src/core/gateway/config/gateway_class/conf_handler_impl.rs` | 18/24 | `get_gateway_class_by_name`, `list_gateway_classes` | Re-exported through `config/mod.rs`; used transitively in tests and `conf_client`. `#[allow]` is stale if the re-export chain resolves the lint. |
| `src/core/gateway/config/edgion_gateway/conf_handler_impl.rs` | 19/25 | `get_edgion_gateway_config_by_name`, `list_edgion_gateway_configs` | Same pattern as above. |
| `src/core/gateway/config/gateway_class/mod.rs` | 4 | `#[allow(unused_imports)]` on re-export line | Both functions are re-exported but possibly not consumed outside tests; the suppression prevents the re-export itself from triggering warnings. |
| `src/core/gateway/config/edgion_gateway/mod.rs` | 4 | Same as above | Same rationale. |
| `src/core/gateway/link_sys/providers/webhook/resolver.rs` | 157 | `_type_check()` in `#[cfg(test)]` | Type-checking helper, intentionally unused at runtime. |

---

## Dead Plugin Struct Fields (0–1 refs in their module)

All four `TlsRouteContext` fields flagged by the module-local scan (`listener_port`, `tls_id`, `rsc`, `is_mtls`) are **actively used** by callers in `src/core/gateway/routes/tls/` — the scan was too narrow (only looked inside the `tls_route/` plugin subdirectory). No genuinely dead struct fields were found in plugin directories.

The `mirror_log` module fields `entry_type` and `timestamp` on `MirrorLogEntry` are written via `new_entry()` (in the same file) and serialized to JSON via Serde — they are not dead.

---

## Dead Backend Functions (confirmed 0 callers outside module)

| File | Line | Function | Notes |
|------|------|----------|-------|
| `src/core/gateway/backends/health/check/config_store.rs` | 86 | `endpoint_slice_keys()` | Returns keys from the endpoint-slice config map; no call sites found outside the file |
| `src/core/gateway/backends/health/check/config_store.rs` | 90 | `endpoint_keys()` | Returns keys from the per-endpoint config map; same — no call sites |
| `src/core/gateway/backends/policy/backend_tls/backend_tls_policy_store.rs` | 287 | `get_by_namespace(namespace)` | Returns policies filtered by namespace; no callers found in `src/` |
| `src/core/gateway/backends/policy/backend_tls/backend_tls_policy_store.rs` | 282 | `get_all()` | Returns full policy map clone; no callers (only self-referential internal construction) |

Functions that appeared to have 0 refs but are actually used internally (only called from within the same file, e.g. via `self.X()`):
- `record_failure` / `record_success` in `circuit_breaker/store.rs` — called by `record_failure_by_addr` / `record_success_by_addr` in the same file, which ARE used externally.
- `set_endpoint_config` — called by `remove_endpoint_config` in the same file and by tests.
- `all_configured_services` — called by `remove_endpoint_config` internally.
- `replace_data_only` / `update_data_only` in `ep_slice_store.rs` — called from `conf_handler_impl.rs`.
- `with_service` in `service_store.rs` — called from `port_resolver.rs`.
- `register_service` / `unregister_service` / `tracked_service_count` in `status_store.rs` — used in tests and potentially in manager code.

---

## Dead Plugin Structs (0 external refs)

| File | Line | Struct | Notes |
|------|------|--------|-------|
| `src/core/gateway/plugins/http/request_mirror/mirror_log.rs` | 7 | `MirrorLogEntry` | 0 refs **outside its own file** per grep filter, but the struct IS used internally by `new_entry()` and `emit_mirror_log()` which are called from `plugin.rs`. Not truly dead — the grep filter excluded the containing file. |

No plugin structs with zero genuine callers were found. The `AllEndpointStatusGlobalConfig` (1 external ref) is referenced from `cli/config.rs` and used by the plugin trait. `ConditionEvalResult` (1 external ref) is re-exported via `conditions/mod.rs`.

---

## Confirmed Dead / Stale `#[allow]` Annotations (can be cleaned up)

These `#[allow(dead_code)]` annotations are on items that **are used** — the suppression is stale noise:

| File | Line | Item | Action |
|------|------|------|--------|
| `src/core/gateway/runtime/server/error_response.rs` | 189 | `end_response_421` | Remove `#[allow]` — function is called |
| `src/core/gateway/runtime/store/gateway.rs` | 41 | `get_gateway` | Remove `#[allow]` — function is called |

---

## Summary

| Category | Count |
|----------|-------|
| Total `.rs` files scanned | 384 |
| `#[allow(dead_code/unused)]` suppressions found | 28 |
| Suppressions that are stale (item is actually used) | 2 (`end_response_421`, `get_gateway`) |
| Suppressions that are correct/intentional | 26 |
| Backend functions with 0 callers outside module | 4 (`endpoint_slice_keys`, `endpoint_keys`, `get_by_namespace`, `get_all`) |
| Top-level gateway pub fns with 0 callers | 2 (`parse_policies`, `parse_service_key`) |
| Dead type alias | 1 (`StringDataSender`) |
| Dead method | 1 (`Parser::span_at`) |
| **Total actionable candidates** | **~10** |

### Recommended removals (highest confidence)

1. **`parse_policies` and `parse_service_key`** in `lb/lb_policy/config.rs` — unused utilities, safe to delete.
2. **`StringDataSender` type alias** in `link_sys/runtime/data_sender.rs` — unused, delete the alias.
3. **`Parser::span_at`** in `plugins/http/dsl/lang/parser/mod.rs` — never called, delete.
4. **`end_response_403`** in `runtime/server/error_response.rs` — dead helper (403 is returned via plugins, not this helper); delete or keep for future use.
5. **`endpoint_slice_keys()` and `endpoint_keys()`** in `backends/health/check/config_store.rs` — dead accessor methods, safe to remove if the health-check manager doesn't plan to use them.
6. **`get_by_namespace()` and `get_all()`** in `backends/policy/backend_tls/backend_tls_policy_store.rs` — dead query methods.
7. **Stale `#[allow(dead_code)]`** on `end_response_421` and `get_gateway` — remove the attribute (items are used).
