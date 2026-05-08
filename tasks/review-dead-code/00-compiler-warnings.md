# Compiler Dead Code Warnings

## Compiler-Flagged Dead Items

**None.** `cargo check --bin edgion-gateway --bin edgion-controller --bin edgion-cli --bin edgion-center --lib` emits zero warnings. All dead-code warnings are suppressed via `#[allow(...)]` attributes throughout the codebase.

---

## `#[allow(dead_code)]` Suppressions

### Module-Level Suppressions (`#![allow(dead_code)]`)

- `src/core/gateway/tls/store/cert_matcher.rs:1` — entire module silenced. The module is actively used (`match_sni`, `set_tls_cert_matcher`, `listener_count` are all called), so the blanket suppression is masking internal private items (`TlsCertMatcherData`, `TlsCertMatcherData::new`) that the compiler considers dead because they are not exported.

### Item-Level Suppressions (`#[allow(dead_code)]`)

#### Compile-time check helpers (intentional — suppression is correct)
- `src/types/resource/kind.rs:68` — `const fn _compile_time_sync_check()`: intentional exhaustive-match guard; never called at runtime.
- `src/types/resource/macros.rs:177` — `const fn _exhaustive_check()` inside macro expansion: same pattern.

#### Test-only code (suppression is in test context)
- `src/core/gateway/cache/lru.rs:456` — `name: &'static str` field in `TtlScenario` test struct; kept for readability.
- `src/core/gateway/plugins/runtime/conditional_filter.rs:244` — `fn run_count()` in `MockRequestFilter` test helper; useful for future test assertions.

#### Planned / not-yet-wired API methods (suppression has comment explaining intent)
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/secret_utils/secret_store.rs:65` — `fn count()` — comment: "Task 12 debug handler".
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/secret_utils/secret_store.rs:71` — `fn keys()` — same note.
- `src/core/gateway/conf_sync/conf_client/config_client.rs:550` — `fn client_cache_keys()` — comment: "validator binary Task 15/16".
- `src/core/gateway/plugins/http/plugin_store.rs:117` — `fn keys_by_kind()` — comment: "Task 15".
- `src/core/gateway/plugins/runtime/plugin_metadata/conf_handler.rs:40` — `fn plugin_metadata_keys()` — comment: "Task 15/16".
- `src/core/gateway/plugins/runtime/plugin_metadata/conf_handler.rs:102` — `fn keys()` — same comment.
- `src/core/gateway/plugins/stream/stream_plugin_store.rs:121` — `fn keys_by_kind()` — comment: "Task 15".

#### Genuinely unused / only partially wired
- `src/core/center/fed_sync/server/mod.rs:50` — `sync_version: u64` field — stored in struct, never read outside.
- `src/core/controller/conf_sync/conf_server/client_registry.rs:23,25` — `client_name: String` and `connected_at: SystemTime` fields in `ClientInfo` — struct fields written but never read.
- `src/core/controller/conf_sync/conf_server/grpc_server.rs:57` — `fn with_static_server()` — test/debug constructor, not used in any test currently.
- `src/core/controller/conf_sync/conf_server/grpc_server.rs:160` — `async fn serve_with_reflection()` — alternate server entrypoint, not wired into any binary.
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handler.rs:145` — `pub struct DefaultHandler` — used in tests only (processor.rs:790/810).
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/backend_tls_policy.rs:595` — `fn secret_has_ca_cert()` — private helper, possibly no longer called.
- `src/core/controller/api/types.rs:43` — `fn is_ready()` — public method, not called anywhere.
- `src/core/ctl/cli/client.rs:20` — `socket_path: Option<PathBuf>` field — stored but never read after construction.
- `src/core/gateway/link_sys/providers/webhook/resolver.rs:157` — `async fn _type_check()` — underscore prefix signals intentional type-check-only helper; never called.
- `src/core/gateway/link_sys/runtime/data_sender.rs:45` — `pub type StringDataSender` — type alias defined but never referenced.
- `src/core/gateway/config/gateway_class/conf_handler_impl.rs:18,24` — `fn get_gateway_class_by_name()` and `fn list_gateway_classes()` — public functions not called from any binary; re-exported via mod.rs with `#[allow(unused_imports)]`.
- `src/core/gateway/config/edgion_gateway/conf_handler_impl.rs:19,25` — `fn get_edgion_gateway_config_by_name()` and `fn list_edgion_gateway_configs()` — same as above.
- `src/core/gateway/lb/lb_policy/config.rs:25` — `fn parse_policies()` — public function, not used.
- `src/core/gateway/lb/lb_policy/config.rs:31` — `fn parse_service_key()` — private function, not used.
- `src/core/gateway/runtime/server/error_response.rs:150` — `async fn end_response_403()` — public async fn, not called in any path.
- `src/core/gateway/runtime/server/error_response.rs:189` — `async fn end_response_421()` — public async fn, not called in any path.
- `src/core/gateway/runtime/store/gateway.rs:41` — `fn get_gateway()` — accessor not called.
- `src/core/gateway/tls/store/conf_handler.rs:40` — `fn create_test_tls()` — test helper in non-test code block.
- `src/core/gateway/tls/store/tls_store.rs:249` — `fn rebuild_matcher()` — private method, not called.
- `src/core/gateway/plugins/http/dsl/lang/compiler/mod.rs:33` — `pub(super) depth: u16` — struct field set but never read at runtime.
- `src/core/gateway/plugins/http/dsl/lang/parser/mod.rs:85` — `pub(super) fn span_at()` — method defined but not called.
- `src/core/gateway/plugins/http/openid_connect/plugin.rs:131,134,139` — `id_token`, `refresh_token`, `token_type` fields in deserialized struct — deserialized from JSON but never read.
- `src/core/gateway/plugins/http/openid_connect/plugin.rs:166` — `plugin_namespace: String` field in runtime struct — written at construction, never read.
- `src/core/gateway/plugins/runtime/conditional_filter.rs:43,89,143,189` — four `fn without_conditions()` constructors on filter wrappers — convenience constructors not called anywhere.

---

## `#[allow(unused_imports)]` Suppressions

- `src/core/gateway/config/gateway_class/mod.rs:4` — re-exports `count_gateway_classes`, `get_gateway_class_by_name`, `list_gateway_classes` that are not used by any binary; suppression hides the re-export warning.
- `src/core/gateway/config/edgion_gateway/mod.rs:4` — same pattern for `count_edgion_gateway_configs`, `get_edgion_gateway_config_by_name`, `list_edgion_gateway_configs`.

---

## `#[allow(unused_variables)]` Suppressions

- `src/core/gateway/routes/tls/proxy.rs:186` — `fn extract_sni(#[allow(unused_variables)] stream: &mut Stream)` — `stream` is only used inside `#[cfg(feature = "boringssl")]` block; under rustls feature the parameter goes unused.

---

## Workaround Suppressions (via `let _ = ...`)

- `src/core/gateway/link_sys/providers/webhook/call.rs:631` — `let _ = StdAtomicU32::new(0); // silence unused import warning` — keeps `StdAtomicU32` import live in a test block; the import itself is only needed conditionally.

---

## Commented-Out Code

No substantial commented-out code blocks (`// fn`, `// pub`, `// struct`, `// impl`, `// async fn`) were found in `src/`. The codebase is clean in this regard.

---

## Summary

- **Total compiler warnings**: 0 (all suppressed)
- **Module-level `#![allow(dead_code)]` suppressions**: 1 (`cert_matcher.rs`)
- **Item-level `#[allow(dead_code)]` suppressions**: 43 occurrences across 25 files
  - Intentional compile-time guards: 2
  - Test-only helpers: 2
  - Planned/not-yet-wired (have task comments): 7
  - Genuinely unused or partially wired (candidates for removal/wiring): 32
- **`#[allow(unused_imports)]` suppressions**: 2
- **`#[allow(unused_variables)]` suppressions**: 1 (cfg-conditional, legitimate)
- **`let _ = ...` workarounds**: 1
- **Total commented-out code candidates**: 0

### Highest-Priority Candidates for Removal or Wiring

1. `StringDataSender` type alias — defined, never used, no comment.
2. `fn parse_service_key()` — private, not called.
3. `fn rebuild_matcher()` in `tls_store.rs` — private, not called.
4. `fn end_response_403()` / `fn end_response_421()` in `error_response.rs` — public, not reachable.
5. `fn get_gateway()` in `gateway.rs` runtime store — accessor not called.
6. `fn create_test_tls()` in `conf_handler.rs` — test helper in non-test code.
7. `client_name` / `connected_at` in `ClientInfo` — written but never read.
8. `parse_policies()` in `lb_policy/config.rs` — public, unreferenced.
9. `gateway_class` / `edgion_gateway` listing functions + their `#[allow(unused_imports)]` re-exports — either wire into API or remove.
10. OpenID Connect `id_token`, `refresh_token`, `token_type`, `plugin_namespace` fields — deserialized/stored but never consumed.
