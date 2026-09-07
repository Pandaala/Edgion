---
name: link-overview
description: Controller↔Gateway gRPC sync protocol — Proto definitions, sync flow, version tracking, resource change events, module distribution.
---

# Sync protocol overview

Controller and Gateway sync configuration via the `ConfigSync` gRPC service. After startup, Gateway acts as the client and connects to Controller; it first fetches server info, then performs List (full) + Watch (incremental) sync for each resource kind.

## Proto definition

The proto file lives at `edgion-common/src/conf_sync/proto/config_sync.proto`:

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
  2. Spawn one sync loop per supported Gateway cache kind
  3. Each loop: List(kind) → one JSON Vec snapshot + sync_version
  4. CacheData.reset(snapshot) → ConfHandler.full_set() → mark the kind ready
  5. Watch(kind, from_version=list.sync_version) → incremental changes
```

The per-kind loops run concurrently. Every loop performs a new full List before its Watch on
startup and after a watch retry/reconnect; it does not resume from a Gateway-retained version across
reconnects. `InitStart`/`InitAdd`/`InitDone` are Controller-side `ServerCache` construction events,
not frames in the List RPC.

### Controller reload flow

```
Controller reload:
  1. Controller generates a new server_id
  2. Existing Watch detects the provider/server_id change and ends with reload metadata
  3. Gateway detects the server_id change
  4. Gateway re-runs List for all kinds (full re-sync)
```

When Controller performs a reload (for example, reconnecting to the K8s API Server in K8s mode), it generates a new `server_id` and notifies all Gateway instances via the Watch stream. After receiving the error, Gateway triggers a full relist to ensure configuration consistency.

### server_id mismatch detection

List and Watch validate a non-empty `expected_server_id` and return gRPC
`failed_precondition` on mismatch. The per-kind retry loop relists against the current provider;
`GetServerInfo` remains startup orchestration rather than a required step for each mismatch.

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

Resource changes are represented by the `ResourceChange` enum, defined in `edgion-common/src/conf_sync/traits.rs`:

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

The default `no_sync_kinds` are:

| Resource | Reason |
|------|------|
| `ReferenceGrant` | Used only for cross-namespace reference validation on the Controller side |
| `Secret` | Used only for certificate/key handling on the Controller side; resolved TLS data is synced via EdgionTls |

An explicit `conf_sync.no_sync_kinds` replaces, rather than extends, this default. Excluded kinds
are removed by `ProcessorRegistry.all_watch_objs()`. Separately, Gateway has no caches for Secret,
ReferenceGrant, or ConfigMap; advertised unsupported cache kinds are skipped. Mandatory
GatewayClass/Gateway/EdgionGatewayConfig caches are injected into effective readiness.

## Operational limits

- A serialized List snapshot larger than 64 MiB is rejected.
- The TLS accept path permits at most 256 in-flight handshakes, with a 10-second handshake timeout
  and a 30-second handoff timeout.
- Internal list/watch forwarding channels are bounded (currently 100 entries).

Large EndpointSlice snapshots that exceed the List cap cannot complete initial sync; diagnose size
and sharding rather than treating the failure as a transient Watch error.

## Channel security (mTLS)

Both gRPC channels support mTLS encryption:

| Channel | Direction | Configuration field |
|------|------|---------|
| **conf_sync** | Controller (server) ↔ Gateway (client) | `conf_sync_security` |
| **federation** | Center (server) ↔ Controller (client) | `center.security` (client) / `grpc_security` (Center server) |

The conf_sync channel **streams resolved Secrets** (TLS private keys, backend
credentials) to the data plane, so its identity story is load-bearing, not optional
hardening. Two ways its certificates come to exist:

- **auto-CA (`conf_sync_security.auto_ca: true`, the shipped Kubernetes default)** — the
  Controller's embedded CA auto-provisions and rotates everything; no operator cert
  handling. This is the section below.
- **file mode (`auto_ca` omitted/false)** — the operator supplies cert/key/ca paths (the
  `ConfSyncSecurityConfig` structure further down). Hot reload still applies; generation
  is external.

### Certificate lifecycle architecture (auto-CA)

This is the *design*; operator-facing knobs, the alerting table, and the emergency
re-key runbook live in
[`skills/02-features/02-config/00-controller-config.md`](../../02-features/02-config/00-controller-config.md)
(the single canonical operator source — do not duplicate it here).

**Material.** An embedded CA (rcgen, ECDSA P-256) is persisted in the
`edgion-conf-sync-ca` Secret (controller-only RBAC). It signs two leaves: a **shared
gateway client leaf** (the `edgion-conf-sync-certs` Secret every gateway mounts) and a
**per-replica controller server leaf** (minted in memory, never persisted). Code:
`edgion-controller/src/services/conf_sync_ca/`.

**Two reconcilers, deliberately split — this is the crux of correctness under HA:**

| Loop | Scope | Job | Why |
|------|-------|-----|-----|
| Rotation reconciler (`rotation.rs`) | **leader only** | The single *writer*: rotates both leaves, same-key CA renewal, writes Secrets | Secret writes must be serialized to one writer |
| Identity sync (`identity_sync.rs`) | **every replica** (NOT leader-gated) | Keeps *this replica's own* serving identity chained to the current root; **read-only** on Secrets | Under `ha_mode: all-serve` every replica serves the API, so every replica must reconcile its own trust anchor — not just the leader |

The per-replica loop mirrors istiod's `watchRootCertAndGenKeyCert` (runs on every pod,
polls the root, re-signs that pod's own serving cert; istio leader-elects only
controllers that must not run concurrently, never the cert path). It is what makes
`auto_ca` safe under **both** `ha_mode` values. A frozen-at-boot server identity on a
non-leader — which is what this loop prevents — would let a revoked client leaf keep
authenticating after a re-key. Convergence is **eventual, not instantaneous**: a
non-leader tears its sessions down when *its own* tick runs (≤ one 60s interval), and a
tick that is failing is surfaced by `cert_identity_sync_failed` (see the config doc's
alerting table) — that metric is precisely the signal that revocation has not yet reached
a pod. The leader-only CA bootstrap is split-brain-safe by construction: `ensure_ca`
POST-creates and adopts on a 409, so two replicas racing at cold start converge on one
root.

**Hot reload, both ends, no process restarts.** Controller: an owned TCP accept loop
feeds tonic `serve_with_incoming_shutdown` from an `ArcSwap<ServerIdentity>`, so the
listener is never rebound and the identity swaps atomically
(`conf_sync/conf_server/{tls_identity,grpc_server,cert_watch}.rs`). Gateway: a `notify`
file-watch (+ 5-min re-stat backstop) rebuilds the client Channel and cancels old streams
via a generation token (`conf_sync/conf_client/{grpc_client,cert_watch}.rs`).

**Rotation vs. re-key — the security hinge.** Certificates are consumed exactly once, at
handshake, so swapping the client-CA verifier governs only *future* handshakes.

- **Leaf rotation (same trust root)** → established sessions are **kept**. The peers on
  them are still authorized; zero-disruption is the point (matches Istio/Envoy SDS, which
  also let existing connections finish on the old cert).
- **Root change (re-key — the only true revocation, or a BYO-root swap)** → every
  controller **tears its existing sessions down**. `ServerIdentityHandle::store()`
  compares the incoming CA bytes to the held ones and, iff the root changed, bumps a trust
  generation and cancels a `CancellationToken`; each accepted connection is wrapped in a
  `TrustScopedStream` that holds the token captured at accept time and fails the socket
  (`ConnectionAborted`) the instant it is cancelled. Without this, an attacker on an
  already-open connection with a leaked leaf would keep streaming Secrets forever after a
  re-key — the client-CA swap alone never re-authenticates a live session. There is
  deliberately **no** `max_connection_age` backstop: a blanket age cap would recycle every
  healthy connection (a full relist each time, since gateways share one client-leaf Secret)
  for no benefit outside a re-key. Emergency re-key is driven by
  `cicd/deploy/kubernetes/scripts/rekey_conf_sync_ca.sh`, which proves the teardown on
  every run (holds an old-key session across the re-key and asserts it dies).

### Configuration structure

```rust
// edgion-common/src/conf_sync/config.rs
pub struct ConfSyncSecurityConfig {
    pub active: Option<String>,          // Name of the active certificate set
    pub certs: Vec<ConfSyncTlsConfig>,   // Multi-cert sets ([[...certs]] array)
    pub tls: Option<ConfSyncTlsConfig>,  // Pre-release removal debt; do not use
    pub auto_ca: bool,                   // Kubernetes internal-CA provisioning
    pub rotation: Option<ConfSyncRotationConfig>, // controller-only rotation policy
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
3. Current code falls back to `tls`; this is removal debt, not a supported configuration contract
4. None configured → no resolved TLS set; startup fail-close rejects this unless `auto_ca` is the
   selected provisioning mode

Path resolution is handled via `ConfSyncTlsConfig::resolve_path()` based on `work_dir`, consistent with the behavior of other configuration paths.

### YAML configuration examples

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

**Center federation channel (Center server side, configured in the separate EdgionCenter repository):**
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
  address: "https://center.example.com:12251"
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
| `auto_ca: true` (K8s default) | mTLS; embedded CA self-provisions + rotates certs | mTLS after the provisioned files appear |
| File-mode TLS config + valid certificates | mTLS gRPC | mTLS connection |
| File-mode TLS config + missing certificates | **Refuses to start** (strict mode) | Connection fails, retries with backoff |
| No security configured (section omitted entirely) | **Refuses to start** (fail-close) | **Refuses to start** (fail-close) |
| `skip_tls = true` | Plaintext + ERROR log + `skip_tls` metric | Plaintext + ERROR log + `skip_tls` metric |

An explicitly active `skip_tls: true` set is the emergency override even when `auto_ca` is also
true; `auto_ca` does not silently cancel that explicit choice.

> **Fail-close since 2026-07-14 (breaking).** This channel carries resolved Secrets
> (TLS private keys, backend credentials), so an omitted `conf_sync_security` no longer
> downgrades to plaintext — both binaries exit non-zero with an actionable error naming
> the three ways forward (`auto_ca`, cert paths, or `skip_tls`). `skip_tls: true` is the
> ONLY way to run this channel in plaintext, and it stays loud. The retired
> `plaintext_default` metric reason is gone; alert on `reason="skip_tls"`.
> Decision record: [skills/04-review/conf-sync/regression-guards/conf-sync-plaintext-default-observable.md](../../04-review/conf-sync/regression-guards/conf-sync-plaintext-default-observable.md).

### Shared TLS layer

`edgion-common/src/grpc_tls.rs`:

| Function | Purpose |
|------|------|
| `load_client_tls(cfg)` | Read ca/cert/key, return `ClientTlsConfig` (file-mode client side) |
| `validate_identity(cert, key, ca)` | Validate-before-swap gate: chain + key-match check every hot-reload / identity swap runs before installing new material |

> Note: there is intentionally **no** `load_server_tls` builder. The controller server
> side goes through `ServerIdentity::from_pems` → `validate_identity` → the owned rustls
> accept loop, so no server-TLS config is ever built without validation. (A former
> unvalidated `load_server_tls_from_pem` was deleted for exactly this reason.)

### Implementation locations

| Component | File | Key function |
|------|------|---------|
| Validate-before-swap gate | `edgion-common/src/grpc_tls.rs` | `validate_identity()` |
| File-mode client TLS | `edgion-common/src/grpc_tls.rs` | `load_client_tls()` |
| conf_sync server (accept loop + identity swap) | `edgion-controller/src/conf_sync/conf_server/{grpc_server,tls_identity,cert_watch}.rs` | `serve_with_shutdown(listener, security, tls_handle, ..)`, `ServerIdentityHandle`, `TrustScopedStream` |
| conf_sync client (hot reload) | `edgion-gateway/src/conf_sync/conf_client/{grpc_client,cert_watch}.rs` | `new(..)`, `reload_client_tls()` |
| auto-CA lifecycle | `edgion-controller/src/services/conf_sync_ca/{ca,secret_io,provision,rotation,identity_sync,cert_util}.rs` | `provision()`, `rotation` reconciler, `identity_sync` reconciler |
| Controller CLI wiring | `edgion-controller/src/cli/mod.rs` | `start_services(..)` provisions, sets `global_handle`, starts `identity_sync` unconditionally; `rotation` is wired via `rotation::init` and *started* by the ConfMgr leadership callback (`center.rs`) |
| Gateway CLI | `edgion-gateway/src/cli/mod.rs` | `create_config_sync_client()` passes `&config.conf_sync_security` |
| Federation Client TLS | `edgion-controller/src/fed_sync/fed_client/mod.rs` | `connect_and_run()` uses `config.security` |

The Center server implementation is maintained in the separate EdgionCenter repository.

### Certificate generation

The `edgion-cli gen-certs` subcommand uses rcgen to generate a complete certificate bundle:

- **CA**: self-signed ECDSA P-256, CN=edgion-grpc-ca
- **Server cert**: CN=edgion-controller, SAN includes localhost/127.0.0.1/::1 + K8s FQDN, EKU=ServerAuth
- **Client cert**: CN=edgion-gateway, EKU=ClientAuth

`--cert-name <NAME>` (default `primary`) parameter:
- **File mode**: certificates are written to the `<output_dir>/<cert-name>/` subdirectory, supporting multiple certificate sets coexisting
- **K8s mode**: Secrets are named `{prefix}-controller-tls-{cert-name}` / `{prefix}-gateway-tls-{cert-name}`

Certificate generation logic: `edgion-cli/src/certs/mod.rs`
Subcommand entry: `edgion-cli/src/cli/commands/gen_certs.rs`

### Integration testing

`edgion-tests/integration/scripts/conf-sync/run_conf_sync_mtls_test.sh` — end-to-end mTLS test, verifying certificate generation → configuration injection → mTLS startup → configuration sync → plaintext rejection. See [05-testing/04-conf-sync-leak-testing.md](../../05-testing/04-conf-sync-leak-testing.md) for details.

## Module distribution

| Module | Path | Responsibility |
|------|------|------|
| **common/conf_sync** | `edgion-common/src/conf_sync/` | Proto definitions, shared traits (`CacheEventDispatch`, `ConfHandler`, `ResourceChange`), shared event/list/Watch types |
| **common/conf_sync::config** | `edgion-common/src/conf_sync/config.rs` | `ConfSyncSecurityConfig`, `ConfSyncTlsConfig` configuration structs |
| **controller/conf_sync** | `edgion-controller/src/conf_sync/` | Server-side implementation: `ConfigSyncServer`, `ConfigSyncGrpcServer` (with mTLS), `ClientRegistry`, `WatchObj` trait |
| **gateway/conf_sync** | `edgion-gateway/src/conf_sync/` | Client-side implementation: `ConfigSyncClient` (with mTLS), `ClientCache<T>`, `CacheData`, event_dispatch |
| **ctl/certs** | `edgion-cli/src/certs/mod.rs` | Certificate generation (rcgen): `generate_certs()` → `CertBundle` |

### Key files

- `edgion-common/src/conf_sync/proto/config_sync.proto` — Proto definitions
- `edgion-common/src/conf_sync/traits.rs` — `ResourceChange`, `CacheEventDispatch`, `ConfHandler`
- `edgion-controller/src/conf_sync/conf_server/config_sync_server.rs` — `ConfigSyncServer`
- `edgion-controller/src/conf_sync/conf_server/grpc_server.rs` — `ConfigSyncGrpcServer`, `ConfigSyncServerProvider`
- `edgion-controller/src/conf_sync/conf_server/client_registry.rs` — `ClientRegistry`
- `edgion-controller/src/conf_sync/conf_server/traits.rs` — `WatchObj`, `WatchResponseSimple`
- `edgion-gateway/src/conf_sync/conf_client/grpc_client.rs` — `ConfigSyncClient`
- `edgion-gateway/src/conf_sync/cache_client/cache.rs` — `ClientCache<T>`, `DynClientCache`
- `edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs` — Event dispatch logic
