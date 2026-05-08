---
name: link-overview
description: Controller↔Gateway gRPC sync protocol — Proto definitions, sync flow, version tracking, resource change events, module distribution.
---

# Sync protocol overview

Controller and Gateway sync configuration via the `ConfigSync` gRPC service. After startup, Gateway acts as the client and connects to Controller; it first fetches server info, then performs List (full) + Watch (incremental) sync for each resource kind.

## Proto definition

The proto file lives at `src/core/common/conf_sync/proto/config_sync.proto`:

```protobuf
service ConfigSync {
    rpc GetServerInfo(ServerInfoRequest) returns (ServerInfoResponse);
    rpc List(ListRequest) returns (ListResponse);
    rpc Watch(WatchRequest) returns (stream WatchResponse);
    rpc WatchServerMeta(WatchServerMetaRequest) returns (stream ServerMetaEvent);
}
```

### 4 RPC methods

| RPC | Direction | Description |
|-----|------|------|
| **GetServerInfo** | Unary | Returns `server_id` and the `supported_kinds` list |
| **List** | Unary | Fetches a full snapshot of resources for a kind, returns JSON data + `sync_version` + `server_id` |
| **Watch** | Server stream | Continuously receives incremental change events for a kind, carries `from_version` to support resumption |
| **WatchServerMeta** | Server stream | Receives server metadata changes (`gateway_instance_count`) for cluster-level awareness |

### Key message fields

| Message | Key fields |
|------|---------|
| `ListRequest` | `kind`, `expected_server_id` |
| `ListResponse` | `data` (JSON), `sync_version`, `server_id` |
| `WatchRequest` | `kind`, `client_id`, `client_name`, `from_version`, `expected_server_id` |
| `WatchResponse` | `data` (JSON), `sync_version`, `err`, `server_id` |
| `ServerInfoResponse` | `server_id`, `supported_kinds` |
| `ServerMetaEvent` | `server_id`, `gateway_instance_count`, `timestamp` |

## Sync flow

### Gateway startup sync

```
Gateway startup:
  1. GetServerInfo() → fetch server_id, supported_kinds
  2. For each kind: List(kind) → full snapshot (InitStart → InitAdd × N → InitDone)
  3. For each kind: Watch(kind, from_version) → continuously receive incremental changes
```

Gateway uses `supported_kinds` to determine which resource types to sync, then issues List and Watch calls one by one. The `sync_version` returned by List is used as the `from_version` for Watch, providing seamless continuation.

### Controller reload flow

```
Controller reload:
  1. Controller generates a new server_id
  2. Watch stream sends a WATCH_ERR_SERVER_RELOAD error
  3. Gateway detects the server_id change
  4. Gateway re-runs List for all kinds (full re-sync)
```

When Controller performs a reload (for example, reconnecting to the K8s API Server in K8s mode), it generates a new `server_id` and notifies all Gateway instances via the Watch stream. After receiving the error, Gateway triggers a full relist to ensure configuration consistency.

### server_id mismatch detection

Both List and Watch requests carry `expected_server_id`. If the `expected_server_id` sent by Gateway does not match Controller's current `server_id`, Controller returns a `WATCH_ERR_SERVER_ID_MISMATCH` error; Gateway will then re-fetch ServerInfo and perform a full sync.

## Version tracking

| Concept | Type | Description |
|------|------|------|
| `sync_version` | `u64` | Globally monotonically increasing version number, generated via `utils::next_resource_version()`; a new value is allocated on every write operation |
| `server_id` | `String` | Controller instance ID (millisecond timestamp), regenerated on every startup or reload |
| `from_version` | `u64` | The last sync version recorded on the Gateway side, used for Watch resumption |
| `expected_server_id` | `String` | Carried by Gateway in List/Watch requests, used to detect Controller restarts |

The version-tracking mechanism guarantees:
- **Incremental sync**: Watch starts pushing events from after `from_version`, avoiding redundant transmissions
- **Stale detection**: when `from_version` is too old (events have been overwritten in the ring buffer), a `TooOldVersion` error is returned, triggering a full re-sync
- **Instance identification**: a `server_id` change indicates a Controller restart or reload, after which Gateway must re-sync in full

## Resource change events

Resource changes are represented by the `ResourceChange` enum, defined in `src/core/common/conf_sync/traits.rs`:

| Event type | Phase | Description |
|---------|------|------|
| `InitStart` | Initialization | Signal: initialization start (log only, carries no resource data) |
| `InitAdd` | Initialization | Add a resource during initialization (writes to the EventStore data, does not enter the ring buffer) |
| `InitDone` | Initialization | Signal: initialization complete; cache may be marked ready |
| `EventAdd` | Runtime | Runtime resource add (writes to ring buffer + notifies watchers) |
| `EventUpdate` | Runtime | Runtime resource update (writes to ring buffer + notifies watchers) |
| `EventDelete` | Runtime | Runtime resource delete (writes to ring buffer + notifies watchers) |

Initialization-phase events (InitStart/InitAdd/InitDone) do not enter the ring buffer; they only update the full snapshot. Runtime events (EventAdd/EventUpdate/EventDelete) are written to both the ring buffer and the snapshot, and trigger watcher notifications.

## Non-synced resources (no_sync_kinds)

The following resource types are not synced to the Gateway via gRPC:

| Resource | Reason |
|------|------|
| `ReferenceGrant` | Used only for cross-namespace reference validation on the Controller side |
| `Secret` | Used only for certificate/key handling on the Controller side; resolved TLS data is synced via EdgionTls |

`no_sync_kinds` are excluded when `ProcessorRegistry.all_watch_objs()` is called; they will not appear in the `supported_kinds` list.

## Channel security (mTLS)

Both gRPC channels support optional mTLS encryption:

| Channel | Direction | Configuration field |
|------|------|---------|
| **conf_sync** | Controller (server) ↔ Gateway (client) | `conf_sync_security` |
| **federation** | Center (server) ↔ Controller (client) | `center.security` (client) / `grpc_security` (Center server) |

### Configuration structure

```rust
// src/core/common/config/mod.rs
pub struct ConfSyncSecurityConfig {
    pub active: Option<String>,          // Name of the active certificate set
    pub certs: Vec<ConfSyncTlsConfig>,   // Multi-cert sets ([[...certs]] array)
    pub tls: Option<ConfSyncTlsConfig>,  // Legacy single-cert format (backward compatible)
}

pub struct ConfSyncTlsConfig {
    pub name: Option<String>, // Certificate set name (required when multiple certs)
    pub cert: String,         // Certificate path (relative to work_dir or absolute)
    pub key: String,          // Private key path
    pub ca: String,           // CA certificate path (verifies peer)
    pub skip_tls: bool,       // Emergency TLS bypass (for failure-mode degradation only)
}
```

`resolve_active_tls()` resolves the effective TLS configuration by priority:
1. `certs` array: find the entry where `name == active`
2. `certs` has only one entry and `active` is None: pick it automatically
3. Fall back to the legacy `tls` field
4. None configured → plaintext gRPC

Path resolution is handled via `ConfSyncTlsConfig::resolve_path()` based on `work_dir`, consistent with the behavior of other configuration paths.

### YAML configuration examples

**Legacy single-cert format (backward compatible):**
```yaml
conf_sync_security:
  tls:
    cert: certs/grpc/server.crt
    key: certs/grpc/server.key
    ca: certs/grpc/ca.crt
```

**Multi-cert format (supports certificate rotation / CA migration):**
```yaml
conf_sync_security:
  active: primary
  certs:
    - name: primary
      cert: certs/grpc/primary/server.crt
      key: certs/grpc/primary/server.key
      ca: certs/grpc/primary/ca.crt
    - name: new-ca
      cert: certs/grpc/new-ca/server.crt
      key: certs/grpc/new-ca/server.key
      ca: certs/grpc/new-ca/ca.crt
```

**Center federation channel (Center server side, `config/edgion-center.yaml`):**
```yaml
grpc_security:
  active: primary
  certs:
    - name: primary
      cert: certs/grpc/primary/server.crt
      key: certs/grpc/primary/server.key
      ca: certs/grpc/primary/ca.crt
```

**Controller federation client (`config/edgion-controller.yaml`):**
```yaml
center:
  address: "https://center.example.com:50052"
  security:
    active: primary
    certs:
      - name: primary
        cert: certs/grpc/primary/client.crt
        key: certs/grpc/primary/client.key
        ca: certs/grpc/primary/ca.crt
```

### Operating modes

| Configuration state | Server (Controller / Center) | Client (Gateway / Controller) |
|----------|------------------------------|-------------------------------|
| No security configured | Plaintext gRPC (backward compatible) | Plaintext connection |
| TLS config + valid certificates | mTLS gRPC | mTLS connection |
| TLS config + missing certificates | **Refuses to start** (strict mode) | Connection fails, retries with backoff |
| `skip_tls = true` | Plaintext + ERROR log | Plaintext + ERROR log |

### Shared TLS loading layer

TLS file reading logic is centralized in `src/core/common/grpc_tls.rs`:

| Function | Purpose |
|------|------|
| `load_server_tls(cfg)` | Read cert/key/ca, return `ServerTlsConfig` |
| `load_client_tls(cfg)` | Read ca/cert/key, return `ClientTlsConfig` |

### Implementation locations

| Component | File | Key function |
|------|------|---------|
| Shared TLS loading | `common/grpc_tls.rs` | `load_server_tls()` / `load_client_tls()` |
| conf_sync Server TLS | `controller/conf_sync/conf_server/grpc_server.rs` | `serve_with_shutdown(security: &ConfSyncSecurityConfig)` |
| conf_sync Client TLS | `gateway/conf_sync/conf_client/grpc_client.rs` | `new(security: &ConfSyncSecurityConfig)` |
| Controller CLI | `controller/cli/mod.rs` | `start_services(security: ConfSyncSecurityConfig)` |
| Gateway CLI | `gateway/cli/mod.rs` | `create_config_sync_client()` passes `&config.conf_sync_security` |
| Center Server TLS | `center/cli/mod.rs` | TLS-aware `tonic::transport::Server::builder()` |
| Federation Client TLS | `controller/fed_sync/fed_client/mod.rs` | `connect_and_run()` uses `config.security` |

### Certificate generation

The `edgion-cli gen-certs` subcommand uses rcgen to generate a complete certificate bundle:

- **CA**: self-signed ECDSA P-256, CN=edgion-grpc-ca
- **Server cert**: CN=edgion-controller, SAN includes localhost/127.0.0.1/::1 + K8s FQDN, EKU=ServerAuth
- **Client cert**: CN=edgion-gateway, EKU=ClientAuth

`--cert-name <NAME>` (default `primary`) parameter:
- **File mode**: certificates are written to the `<output_dir>/<cert-name>/` subdirectory, supporting multiple certificate sets coexisting
- **K8s mode**: Secrets are named `{prefix}-controller-tls-{cert-name}` / `{prefix}-gateway-tls-{cert-name}`

Certificate generation logic: `src/core/ctl/certs/mod.rs`
Subcommand entry: `src/core/ctl/cli/commands/gen_certs.rs`

### Integration testing

`examples/test/scripts/conf-sync/run_conf_sync_mtls_test.sh` — end-to-end mTLS test, verifying certificate generation → configuration injection → mTLS startup → configuration sync → plaintext rejection. See [05-testing/04-conf-sync-leak-testing.md](../../05-testing/04-conf-sync-leak-testing.md) for details.

## Module distribution

| Module | Path | Responsibility |
|------|------|------|
| **common/conf_sync** | `src/core/common/conf_sync/` | Proto definitions, shared traits (`CacheEventDispatch`, `ConfHandler`, `ResourceChange`), shared event/list/Watch types |
| **common/config** | `src/core/common/config/mod.rs` | `ConfSyncSecurityConfig`, `ConfSyncTlsConfig` configuration structs |
| **controller/conf_sync** | `src/core/controller/conf_sync/` | Server-side implementation: `ConfigSyncServer`, `ConfigSyncGrpcServer` (with mTLS), `ClientRegistry`, `WatchObj` trait |
| **gateway/conf_sync** | `src/core/gateway/conf_sync/` | Client-side implementation: `ConfigSyncClient` (with mTLS), `ClientCache<T>`, `CacheData`, event_dispatch |
| **ctl/certs** | `src/core/ctl/certs/mod.rs` | Certificate generation (rcgen): `generate_certs()` → `CertBundle` |

### Key files

- `src/core/common/conf_sync/proto/config_sync.proto` — Proto definitions
- `src/core/common/conf_sync/traits.rs` — `ResourceChange`, `CacheEventDispatch`, `ConfHandler`
- `src/core/controller/conf_sync/conf_server/config_sync_server.rs` — `ConfigSyncServer`
- `src/core/controller/conf_sync/conf_server/grpc_server.rs` — `ConfigSyncGrpcServer`, `ConfigSyncServerProvider`
- `src/core/controller/conf_sync/conf_server/client_registry.rs` — `ClientRegistry`
- `src/core/controller/conf_sync/conf_server/traits.rs` — `WatchObj`, `WatchResponseSimple`
- `src/core/gateway/conf_sync/conf_client/grpc_client.rs` — `ConfigSyncClient`
- `src/core/gateway/conf_sync/cache_client/cache.rs` — `ClientCache<T>`, `DynClientCache`
- `src/core/gateway/conf_sync/cache_client/event_dispatch.rs` — Event dispatch logic
