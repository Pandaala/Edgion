# Dead Code Candidates in src/types/

Scanned: 113 `.rs` files in `src/types/`. Research-only — no source files modified.

---

## Zero / Near-Zero Reference Items

### Constants with zero external references (only defined in `src/types/`, never imported elsewhere)

| File:Line | Constant | Notes |
|-----------|----------|-------|
| `src/types/constants/app.rs:10` | `LOG_PREFIX_OPERATOR` | Defined, never used anywhere in the codebase |
| `src/types/constants/app.rs:13` | `LOG_PREFIX_GATEWAY` | Defined, never used anywhere in the codebase |
| `src/types/constants/app.rs:16` | `DEFAULT_LOG_DIR` | Defined, never used anywhere in the codebase |
| `src/types/constants/app.rs:19` | `DEFAULT_CONFIG_DIR` | Defined, never used anywhere in the codebase |
| `src/types/constants/app.rs:22` | `DEFAULT_WORK_DIR` | Defined, never used anywhere in the codebase |
| `src/types/constants/annotations.rs:62` | `k8s::LAST_APPLIED_CONFIG` | Defined, never used anywhere in the codebase |
| `src/types/constants/labels.rs:13` | `edgion::MANAGED_BY` | Defined, never used anywhere in the codebase |

### Header constants — entire module appears unused externally

All constants in `src/types/constants/headers.rs` have zero external references. The module's constants are defined but the actual code uses raw string literals instead:

| File:Line | Constant | Notes |
|-----------|----------|-------|
| `src/types/constants/headers.rs:8` | `proxy::X_REAL_IP` | 0 external refs |
| `src/types/constants/headers.rs:10` | `proxy::X_FORWARDED_PROTO` | 0 external refs |
| `src/types/constants/headers.rs:12` | `proxy::X_FORWARDED_HOST` | 0 external refs |
| `src/types/constants/headers.rs:14` | `proxy::X_FORWARDED_PORT` | 0 external refs |
| `src/types/constants/headers.rs:20` | `upgrade::UPGRADE` | 0 external refs |
| `src/types/constants/headers.rs:22` | `upgrade::CONNECTION` | 0 external refs |
| `src/types/constants/headers.rs:24` | `upgrade::WEBSOCKET` | 0 external refs |
| `src/types/constants/headers.rs:30` | `cors::ORIGIN` | 0 external refs |
| `src/types/constants/headers.rs:32` | `cors::ACCESS_CONTROL_REQUEST_METHOD` | 0 external refs |
| `src/types/constants/headers.rs:34` | `cors::ACCESS_CONTROL_REQUEST_HEADERS` | 0 external refs |
| `src/types/constants/headers.rs:36` | `cors::ACCESS_CONTROL_ALLOW_ORIGIN` | 0 external refs |
| `src/types/constants/headers.rs:38` | `cors::ACCESS_CONTROL_ALLOW_METHODS` | 0 external refs |
| `src/types/constants/headers.rs:40` | `cors::ACCESS_CONTROL_ALLOW_HEADERS` | 0 external refs |
| `src/types/constants/headers.rs:42` | `cors::ACCESS_CONTROL_MAX_AGE` | 0 external refs |
| `src/types/constants/headers.rs:44` | `cors::ACCESS_CONTROL_ALLOW_CREDENTIALS` | 0 external refs |
| `src/types/constants/headers.rs:46` | `cors::ACCESS_CONTROL_EXPOSE_HEADERS` | 0 external refs |

Only `proxy::X_FORWARDED_FOR` has 1 external ref. All CORS and upgrade header constants are unused.

### Structs referenced only within `src/types/` — no runtime consumption

| File:Line | Item | Type | Notes |
|-----------|------|------|-------|
| `src/types/resources/gateway.rs:105` | `GatewaySpecTLS` | struct | Field type in `GatewaySpec.tls`, used by name only in gateway.rs; controller accesses via `.tls` field, no runtime gateway code reads its fields |
| `src/types/resources/gateway.rs:117` | `GatewayBackendTLS` | struct | Field in `GatewaySpecTLS.backend`; no code outside `src/types/resources/gateway.rs` ever names this type |
| `src/types/resources/gateway.rs:128` | `GatewayFrontendTLS` | struct | Field in `GatewaySpecTLS.frontend`; only named in gateway.rs itself |
| `src/types/resources/gateway.rs:140` | `GatewayFrontendTLSDefault` | struct | Nested under `GatewayFrontendTLS.default`; only in gateway.rs |
| `src/types/resources/gateway.rs:149` | `GatewayFrontendTLSPerPort` | struct | Nested under `GatewayFrontendTLS.per_port`; only in gateway.rs |
| `src/types/resources/edgion_tls.rs:173` | `EdgionTlsExtend` | struct | Field `EdgionTlsSpec.extend`; no code outside edgion_tls.rs reads `.extend` (controller writes `extend: None`, gateway never touches it) |
| `src/types/resources/edgion_tls.rs:104` | `OcspStaplingConfig` | struct | Field `EdgionTlsExtend.ocsp_stapling`; only defined and nested in edgion_tls.rs |
| `src/types/resources/edgion_tls.rs:202` | `SessionTicketConfig` | struct | Field `EdgionTlsExtend.session_ticket`; same — extend block is never consumed |
| `src/types/resources/edgion_tls.rs:220` | `SessionCacheConfig` | struct | Field `EdgionTlsExtend.session_cache`; same |
| `src/types/resources/edgion_tls.rs:235` | `RevocationCheckConfig` | struct | Field `EdgionTlsExtend.revocation_check`; same |
| `src/types/resources/edgion_tls.rs:245` (enum) | `RevocationMode` | enum | Only inside RevocationCheckConfig which is never read |
| `src/types/resources/edgion_tls.rs:258` | `EarlyDataConfig` | struct | Field `EdgionTlsExtend.early_data`; same — never consumed |
| `src/types/resources/gateway_class.rs:62` | `ParametersReference` | struct | Field `GatewayClassSpec.parameters_ref`; no code outside gateway_class.rs names this type |

### Plugin configs: constants used only within their own file

| File:Line | Constant | Notes |
|-----------|----------|-------|
| `src/types/resources/edgion_plugins/plugin_configs/all_endpoint_status.rs:5` | `MAX_TIMEOUT_MS` | Only used as a clamp in the same file; no outside code imports it |
| `src/types/resources/edgion_plugins/plugin_configs/all_endpoint_status.rs:7` | `MAX_WALL_TIMEOUT_MS` | Same — file-internal only |
| `src/types/resources/edgion_plugins/plugin_configs/all_endpoint_status.rs:9` | `MAX_ENDPOINTS_LIMIT` | Same — file-internal only |
| `src/types/resources/edgion_plugins/plugin_configs/all_endpoint_status.rs:11` | `MAX_BODY_SIZE_LIMIT` | Same — file-internal only |

These four `pub const` items are only referenced by clamping logic in `all_endpoint_status.rs` itself. They are `pub` but never imported by any other module.

---

## Suspicious Enum Variants — Zero Runtime Usage

- `src/types/filters.rs:65` — enum `PluginTags` — **entire enum** (`Auth`, `Security`, `Proxy`, `Traffic`): no variant is matched or constructed anywhere in the codebase. The type exists but is never instantiated or pattern-matched. 0 external refs.

- `src/types/resources/link_sys/elasticsearch.rs:204` — enum `EsVendor` (`OpenSearch`, `Elasticsearch`): only referenced inside a test in elasticsearch.rs itself. No runtime production code reads or constructs this variant.

- `src/types/resources/link_sys/elasticsearch.rs:163` — enum `EsFailedCacheType` (`LocalFile`, `Memory`): only referenced inside a test in elasticsearch.rs itself. No runtime production code uses this.

- `src/types/resources/link_sys/webhook.rs:270` — struct `ActiveHealthCheck` (reachable via `WebhookHealthCheck.active`): 0 external refs. `PassiveHealthCheck` is used (in tests), but `ActiveHealthCheck` is never constructed or read outside its definition.

- `src/types/resources/grpc_route.rs:360` — struct `GRPCRouteRetry` (field `GRPCRouteRule.retry`): only defined and used as a field type inside grpc_route.rs. No gateway or controller code reads `.retry` on a GRPCRoute rule — the GRPC retry path appears to be incomplete/unimplemented.

- `src/types/resources/grpc_route.rs:326` — struct `GRPCRequestMirrorFilter` (field `GRPCRouteFilter.request_mirror`): only defined inside grpc_route.rs. The mirror filter for GRPC is never consumed by any gateway handler.

---

## Unused Constants (additional)

- `src/types/constants/app.rs:4` — `CONTROLLER_NAME`: only referenced in a comment line (`//!`) in constants/mod.rs. Effectively unused.
- `src/types/resource/macros.rs:17` — `CAPACITY_LARGE`: only referenced by a macro arm `(@capacity large)` inside macros.rs itself. No resource type actually uses `large` capacity — only `small` (7 resources) and `normal` (5 resources) are used.

---

## Notes on Items That Look Dead But Are Not

- `GatewaySpecTLS` / `GatewayFrontendTLS` / `GatewayBackendTLS` subtypes: while no code outside `gateway.rs` names these types by name, the controller handler in `handlers/gateway.rs` does access `g.spec.tls` and `spec_tls.frontend` by field. These structs are effectively schema-only types — they deserialize from config but their advanced fields (per-port, backend TLS) are mostly read-path-only in the controller. They are not truly dead in the schema sense, but `GatewayFrontendTLSPerPort` and `GatewayFrontendTLSDefault` are only accessed through the parent and their specific fields are read in the controller.
- `EdgionTlsExtend` and all its nested types (`OcspStaplingConfig`, `SessionTicketConfig`, `SessionCacheConfig`, `RevocationCheckConfig`, `EarlyDataConfig`): these are schema/config types that appear to be future work — the controller always sets `extend: None`, and no gateway code reads the extend block at all.
- `types::observe::LogType` (re-exported as `crate::types::LogType`): there is a separate `LogType` enum in `core/gateway/link_sys/providers/local_file/mod.rs` that is unrelated. The `types::observe::LogType` is re-exported but does not appear to be used directly by name in non-types code (gateway logger_factory imports the local_file one). This warrants further review.

---

## Summary

| Category | Count |
|----------|-------|
| Files scanned | 113 |
| Zero-ref constants (never imported) | 5 (app.rs) + 16 (headers.rs) + 1 (k8s annotation) + 1 (label) = **23** |
| Constants `pub` but only file-internal use | 4 (all_endpoint_status.rs) |
| Structs with zero external references | 13 (TLS extend cluster + gateway TLS sub-structs + ParametersReference) |
| Enums / enum types with zero production usage | 5 (PluginTags, EsVendor, EsFailedCacheType, GRPCRouteRetry, GRPCRequestMirrorFilter) |
| Structs with zero external refs (webhook, GRPC) | 2 (ActiveHealthCheck, GRPCRequestMirrorFilter) |
| **Total dead code candidates** | **~47 items** |

The highest-confidence removals are:
1. The entire `src/types/constants/headers.rs` module (16 constants, completely unused)
2. The five app constants `LOG_PREFIX_OPERATOR`, `LOG_PREFIX_GATEWAY`, `DEFAULT_LOG_DIR`, `DEFAULT_CONFIG_DIR`, `DEFAULT_WORK_DIR`
3. `PluginTags` enum (defined in filters.rs, never instantiated)
4. `EdgionTlsExtend` and its 5 sub-types (schema declared but controller always sets `extend: None`, gateway never reads it)
5. `GRPCRouteRetry` and `GRPCRequestMirrorFilter` (GRPC mirror/retry not implemented in gateway)
