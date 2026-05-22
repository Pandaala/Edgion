# Dead Code Candidates in src/core/center/ and src/core/ctl/

## Center: Dead Functions (0-1 external refs)

All public functions in `center/` are reachable: API handlers are registered in the router
(`api/mod.rs`), internal methods are called from `fed_sync/server`, `aggregator`, `commander`,
`proxy`, and `watch_cache`. No function-level dead code was found.

**Note:** `socket_path` field in `EdgionClient` carries `#[allow(dead_code)]` (compiler-confirmed).
`sync_version` field in an SSE envelope struct in `fed_sync/server/mod.rs:51` also carries
`#[allow(dead_code)]` (reserved for future stale-response detection).

## Center: Dead Structs/Enums (0-1 external refs)

| File:Line | Item | Type | Refs outside file | Notes |
|-----------|------|------|-------------------|-------|
| `src/core/center/fed_sync/registry/mod.rs:12` | `ControllerSession` | struct | 0 | Private implementation detail of `ControllerRegistry`; lives only inside the registry's `HashMap`. It is not `pub`-exported from the crate and cannot be referenced externally. Its `is_online()` method is called only within the same file. Not dead — it is the core internal state type. |
| `src/core/center/fed_sync/registry/mod.rs:163` | `SessionView` | struct | 0 | Returned by `get_session()`. Callers in `commander/mod.rs` and `proxy/mod.rs` call `get_session()` but use the result as an `Option<SessionView>` — they only read its fields. No external code names the type by path. Not dead — it is the data carrier for session reads. |
| `src/core/center/aggregator/mod.rs:139` | `ControllerSummary` | struct | 0 | Returned by `controller_summaries()`. Used extensively inside `api/mod.rs`, `api/region_route_handlers.rs`, `api/global_connection_ip_restriction_handlers.rs`, and aggregator tests. The grep excluded same-file usage and callers go through the method return type, so cross-file callers appear as 0. Not dead. |
| `src/core/center/api/consistency_handlers.rs:26` | `ConsistencyReport` | struct | 0 | Used locally inside `consistency_handlers.rs` for route consistency checks. Not dead — it is constructed and serialized in `cluster_region_routes_consistency` / `service_region_routes_consistency`. |
| `src/core/center/api/consistency_handlers.rs:36` | `ConflictDetail` | struct | 0 | Same file — used as a field of `ConsistencyReport`. Not dead. |
| `src/core/center/api/global_connection_ip_restriction_handlers.rs:33` | `CenterGlobalIpRestrictionView` | struct | 0 | Used as a response type in the same file. Not dead. |
| `src/core/center/api/global_connection_ip_restriction_handlers.rs:42` | `ConsistencyResult` | struct | 0 | Used as a response type in the same file. Not dead. |
| `src/core/center/api/global_connection_ip_restriction_handlers.rs:136` | `ControllerOpResult` | struct | 0 | Internal fan-out helper; used heavily throughout the same file. Not dead. |
| `src/core/center/api/global_connection_ip_restriction_handlers.rs:148` | `FanOutResponse` | struct | 0 | Same file — returned from multiple handler functions. Not dead. |
| `src/core/center/api/global_connection_ip_restriction_handlers.rs:229` | `CreateRequest` | struct | 0 | Deserialized from JSON in `create_global_ip_restriction`. Not dead. |
| `src/core/center/api/global_connection_ip_restriction_handlers.rs:334` | `UpdateRequest` | struct | 0 | Deserialized from JSON. Not dead. |
| `src/core/center/api/global_connection_ip_restriction_handlers.rs:378` | `DeleteRequest` | struct | 0 | Deserialized from JSON. Not dead. |
| `src/core/center/api/global_connection_ip_restriction_handlers.rs:405` | `PatchEnableRequest` | struct | 0 | Deserialized from JSON. Not dead. |
| `src/core/center/api/global_connection_ip_restriction_handlers.rs:492` | `PatchActiveProfileRequest` | struct | 0 | Deserialized from JSON. Not dead. |
| `src/core/center/api/region_route_handlers.rs:20` | `FailoverRequest` | struct | 0 | Deserialized from JSON and used in `fan_out_failover`. Not dead. |
| `src/core/center/config/mod.rs:67` | `CenterServerConfig` | struct | 0 | Embedded as `CenterConfig::server`; fields accessed as `config.server.http_addr` / `config.server.grpc_addr` in `cli/mod.rs`. Not dead. |
| `src/core/center/config/mod.rs:8` | `DatabaseConfig` | struct | 0 | Embedded as `CenterConfig::database`; fields accessed as `config.database.enabled` / `config.database.sqlite_path` in `cli/mod.rs`. Not dead. |
| `src/core/center/metadata_store/mod.rs:21` | `CenterClusterRouteView` | struct | 0 | Returned by `list_cluster_routes()`, consumed by API handlers. Not dead. |
| `src/core/center/metadata_store/mod.rs:31` | `CenterServiceRouteView` | struct | 0 | Returned by `list_service_routes()`. Not dead. |
| `src/core/center/metadata_store/mod.rs:60` | `CenterGlobalIpRestrictionEntryView` | struct | 0 | Returned by `list_global_ip_restrictions()`. Not dead. |

**Conclusion:** All center structs with 0 external-file references are either:
- Internal implementation types used within a single module (e.g., `ControllerSession` inside the registry `HashMap`)
- Return types of methods whose callers use method syntax without naming the type
- Config sub-structs accessed via parent struct fields

None qualify as dead code in the traditional sense.

## CTL: Dead Functions (0-1 refs)

| File:Line | Function | Refs | Notes |
|-----------|----------|------|-------|
| `src/core/ctl/cli/output.rs:93` | `print_message` | 0 | **True dead code.** Defined but never called anywhere in the codebase. Likely was used by an earlier command and left behind after refactoring. |

## CTL: Dead Structs/Enums (0-1 refs)

| File:Line | Item | Type | Refs outside file | Notes |
|-----------|------|------|-------------------|-------|
| `src/core/ctl/cli/commands/gen_certs.rs:10` | `OutputMode` | enum | 0 | Used only within `gen_certs.rs` (as a field of `GenCertsArgs`, matched in execute, and tested). Not dead — it drives gen-certs behavior. |
| `src/core/ctl/cli/output.rs:28` | `ResourceRow` | struct | 0 | Used within `output.rs` itself (`print_resource_list`). Not dead — it is the row type for the tabled display. |

## Center Fed_Sync Dead Code

| File:Line | Item | Refs | Notes |
|-----------|------|------|-------|
| `src/core/center/fed_sync/server/mod.rs:51` | `sync_version` field | — | Compiler-flagged field carrying `#[allow(dead_code)]`; reserved for stale-response detection logic. Not cleanable without design change. |

## Already-Suppressed Dead Code (compiler-known)

| File:Line | Item | Notes |
|-----------|------|-------|
| `src/core/center/fed_sync/server/mod.rs:50-51` | `SseEnvelope::sync_version` field | `#[allow(dead_code)]` — reserved field |
| `src/core/ctl/cli/client.rs:20-21` | `EdgionClient::socket_path` field | `#[allow(dead_code)]` — Unix socket path, not yet wired to reqwest transport |

## Summary

Total `.rs` files scanned: 33 (19 in `center/`, 14 in `ctl/`)

Total genuine dead code candidates found: **1 function**

| Item | Location | Confidence | Action |
|------|----------|------------|--------|
| `print_message` | `src/core/ctl/cli/output.rs:93` | High — 0 callers project-wide | Safe to remove |

All other "0 external refs" hits were false positives caused by:
- Types used exclusively within the same module (private implementation details)
- Return types referenced via method calls, not by name at the call site
- Config sub-structs accessed through parent struct field paths
- Items suppressed with `#[allow(dead_code)]` with intentional comments
