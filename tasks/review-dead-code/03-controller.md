# Dead Code Candidates in src/core/controller/

Scanned: all `.rs` files under `src/core/controller/` (≈130 files).  
Method: grep-based reference counting + targeted cross-file verification.

---

## Already Annotated with `#[allow(dead_code)]`

These items the compiler itself flagged; annotations are present but stale or incomplete.

| File:Line | Item | Notes |
|-----------|------|-------|
| `conf_sync/conf_server/grpc_server.rs:58` | `ConfigSyncGrpcServer::with_static_server()` | Annotated dead; no callers found anywhere in `src/` |
| `conf_sync/conf_server/grpc_server.rs:161` | `ConfigSyncGrpcServer::serve_with_reflection()` | Annotated dead; no callers found anywhere in `src/` |
| `conf_sync/conf_server/client_registry.rs:24,26` | `ClientMeta::client_name`, `ClientMeta::connected_at` | Annotated dead; struct fields written but never read |
| `conf_mgr/sync_runtime/resource_processor/handler.rs:146` | `DefaultHandler` struct | Annotated dead; only used inside `#[cfg(test)]` in `processor.rs` |
| `conf_mgr/sync_runtime/resource_processor/secret_utils/secret_store.rs:66` | `SecretStore::count()` | Annotation says "Task 12 pending" but **is already consumed** by `debug_handlers.rs:37` — annotation is stale, can be removed |
| `conf_mgr/sync_runtime/resource_processor/secret_utils/secret_store.rs:72` | `SecretStore::keys()` | Annotated dead; truly unused in production — only referenced in an `#[allow(dead_code)]`-tagged test assertion |
| `conf_mgr/sync_runtime/resource_processor/handlers/backend_tls_policy.rs:597` | `BackendTlsPolicyHandler::secret_has_ca_cert()` | Annotated dead; no callers found anywhere |
| `api/types.rs:44` | `AdminState::is_ready()` | Annotated dead; never called in production |

---

## Functions with 0 External References (not annotated)

| File:Line | Function | Notes |
|-----------|----------|-------|
| `conf_mgr/sync_runtime/resource_processor/status_utils.rs:376` | `accepted_condition_with_message()` | `pub fn` but **only called once** by `accepted_condition()` in the same file; not re-exported; effectively a private helper — should be `pub(super)` or merged into caller |
| `conf_mgr/sync_runtime/resource_processor/secret_utils/secret_ref.rs:15` | `new_secret_ref_manager()` | `pub fn` but **only used in `#[cfg(test)]`** blocks; production code always calls `SecretRefManager::new()` or `SecretRefManager::with_component()` directly |

---

## Structs/Enums with Effectively No External Consumers

| File:Line | Item | Type | Notes |
|-----------|------|------|-------|
| `conf_mgr/sync_runtime/resource_processor/ref_manager.rs:124` | `RefManagerStats` | struct | Returned by `BidirectionalRefManager::stats()` and used in tests + one `revalidation_listener.rs` log call; exported broadly via `pub use` chain but the only non-test consumer is a tracing log — low value export |
| `conf_sync/conf_server/config_sync_server.rs:17` | `EventDataSimple` | struct | Used internally within `config_sync_server.rs`; re-exported through `conf_sync/mod.rs` but consumed only in `gateway/conf_sync/` — the controller-side `pub use` in `conf_sync/mod.rs` seems unnecessary |
| `conf_sync/conf_server/config_sync_server.rs:26` | `ListDataSimple` | struct | Same situation as `EventDataSimple`; re-exported in controller `conf_sync/mod.rs` but the real consumers are all in `gateway/conf_sync/` |

---

## Stale `#[allow(dead_code)]` Annotations (Item Is Now Live)

| File:Line | Item | Issue |
|-----------|------|-------|
| `conf_mgr/sync_runtime/resource_processor/secret_utils/secret_store.rs:65` | `SecretStore::count()` | Comment says "allow until Task 12" but `count()` is already called in `api/debug_handlers.rs:37`; annotation should be removed |

---

## Observations on Module-Level Re-exports

The `conf_sync/mod.rs` re-exports a wide surface that is mostly consumed by the **gateway** side, not the controller itself. Items like `ClientRegistry`, `ConfigSyncGrpcServer`, `ConfigSyncServerProvider`, `WatchObj`, `WatchResponseSimple`, `EventDataSimple`, `ListDataSimple` are all forwarded — some of which (e.g. `ClientRegistry`) appear to have no production callers outside tests. Worth auditing whether the controller's `conf_sync/mod.rs` re-exports could be narrowed.

---

## Summary

- Total `.rs` files scanned: ~130
- Confirmed dead (compiler-annotated, no callers): **7 items**
  - `with_static_server`, `serve_with_reflection`, `ClientMeta` fields, `DefaultHandler`, `SecretStore::keys()`, `secret_has_ca_cert`, `AdminState::is_ready()`
- Stale annotation (item is now live): **1** — `SecretStore::count()`
- Unannotated dead/near-dead: **2 functions** — `accepted_condition_with_message`, `new_secret_ref_manager`
- Structs with low-value wide exports: **3** — `RefManagerStats`, `EventDataSimple`, `ListDataSimple`
- No entirely dead resource-handler files were found; all handlers under `handlers/` are registered in at least one controller variant.
