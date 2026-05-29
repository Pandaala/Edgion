---
name: center-fed-sync-server
description: FederationGrpcServer — RegisterRequest validation contract, bidirectional stream protocol, error handling, and capacity caps.
---

# Federation gRPC Server

Source: `src/core/center/fed_sync/server/mod.rs`

## Validation caps (canonical reference)

These constants are defined at `src/core/center/fed_sync/server/mod.rs:46-57` and are the
**authoritative caps** for the federation gRPC surface.

| Field | Cap | Constant |
|-------|-----|----------|
| Total registered controllers (online + offline) | 10 000 | `MAX_REGISTRY_ENTRIES` |
| `controller_id` length | 253 bytes | `MAX_CONTROLLER_ID_LEN` |
| `cluster` length | 63 bytes | `MAX_CLUSTER_LEN` |
| Per item in `env`, `tag`, `supported_kinds` | 63 bytes | `MAX_TAG_LEN` |
| Items in `env`, `tag`, `supported_kinds` lists | 32 | `MAX_LIST_ITEMS` |

All caps follow K8s label-value conventions (63 bytes per token, 253 bytes for full
DNS-style identifiers).

> For the security rationale, see the `common-center-03` finding, recorded one-file-per-finding under the relevant `skills/04-review/<topic>/` directory (use `grep`/`ls` to find it).

## `validate_register_req` — validation function

```
fn validate_register_req(req: &RegisterRequest) -> Result<(), &'static str>
```

Source: `src/core/center/fed_sync/server/mod.rs:66-86`

Order of checks:
1. `controller_id` must not be empty.
2. `controller_id.len()` must be ≤ `MAX_CONTROLLER_ID_LEN` (253).
3. `controller_id` must contain no control characters.
4. `cluster.len()` must be ≤ `MAX_CLUSTER_LEN` (63). Empty cluster is **allowed**
   (aggregator normalizes it to `"unknown"`).
5. `cluster` must contain no control characters.
6. `validate_string_list` on `env`, `tag`, `supported_kinds`:
   - List length ≤ `MAX_LIST_ITEMS` (32).
   - Each item ≤ `MAX_TAG_LEN` (63) bytes.
   - Each item contains no control characters.

Rejection reasons are logged via `tracing::warn!` at the call site
(`src/core/center/fed_sync/server/mod.rs:239-251`) but the peer-facing
`Status::invalid_argument` message is a fixed string — attacker-controlled bytes
are never echoed back.

## Capacity gate

```
fn registry_capacity_exceeded(registry: &ControllerRegistry, incoming_id: &str, cap: usize) -> bool
```

Source: `src/core/center/fed_sync/server/mod.rs:122-124`

Returns `true` only when `registry.len() >= cap` **and** the `incoming_id` is not
already known. Reconnects from an already-registered controller always succeed,
preserving operator recovery during a flood.

Rejection: `Status::resource_exhausted("Federation registry is at capacity")`
(`src/core/center/fed_sync/server/mod.rs:259`).

## `FederationGrpcServer` struct

Source: `src/core/center/fed_sync/server/mod.rs:181-191`

Fields:
- `registry: ControllerRegistry` — live session map.
- `aggregator: Arc<ResourceAggregator>` — per-cluster controller counts.
- `pending_commands: PendingCommandMap` — one-shot channels for `CommandResponse`.
- `pending_proxies: PendingProxyMap` — one-shot channels for `HttpProxyResponse`.
- `sync_config: CenterSyncConfig` — ping interval, command timeout.
- `sync_client: Arc<CenterSyncClient>` — per-kind watch cache registries.
- `db: Option<Arc<CenterDb>>` — SQLite, absent when `database.enabled = false`.

## Bidirectional stream protocol

The `sync` RPC is a bidirectional streaming call (`src/core/center/fed_sync/server/mod.rs:218`):

```
Controller → Center (ControllerMessage):
  RegisterRequest      — first message; must arrive within 5 s
  Pong                 — heartbeat response
  WatchListResponse    — full snapshot of a resource kind
  WatchEventResponse   — incremental watch event batch
  CommandResponse      — response to a server-initiated command
  HttpProxyResponse    — response to a server-initiated HTTP proxy request

Center → Controller (CenterMessage):
  RegisterAck          — sent immediately after successful registration
  FedWatchRequest      — Center requests a watch for a resource kind (from_version)
  Ping                 — heartbeat sent every ping_interval_secs (default 30 s)
  CommandRequest       — (fan-out) command dispatched from Admin API
  HttpProxyRequest     — (fan-out) proxy request dispatched from Admin API
```

After `RegisterAck`, Center sends a `FedWatchRequest` for `PluginMetaData`
(`src/core/center/fed_sync/server/mod.rs:322-334`), using the last known
`sync_version` from the per-controller cache to avoid unnecessary re-syncs.

### Watch state tracking (`FedWatchState`)

Source: `src/core/center/fed_sync/server/mod.rs:145-176`

Each session tracks a `request_id` (UUID) per kind. Stale responses (wrong `request_id`)
are silently skipped. On server-ID change (Controller restart detected), Center re-watches
from version 0.

## Error handling

| Trigger | Status / behavior |
|---------|-------------------|
| No `RegisterRequest` within 5 s | `Status::deadline_exceeded` |
| First message is not `RegisterRequest` | `Status::invalid_argument` |
| `validate_register_req` fails | `Status::invalid_argument("RegisterRequest validation failed")` — fixed message |
| Registry at capacity (new ID) | `Status::resource_exhausted("Federation registry is at capacity")` |
| Heartbeat timeout (3× `ping_interval`) | `mark_offline_all(HEARTBEAT)`, loop exits |
| Stream error | `mark_offline_all(DISCONNECT)`, loop exits |
| Stream closed | `mark_offline_all(DISCONNECT)`, loop exits |
| `WatchEventResponse` error field set | 3 s backoff, then re-watch from version 0; consecutive errors escalate from INFO → WARN |
| `WatchListResponse` parse error | Metric recorded, warning logged; session continues |

The `mark_offline_all` closure (`src/core/center/fed_sync/server/mod.rs:414-447`)
propagates disconnect to all four state holders in lockstep:
`ControllerRegistry`, `ResourceAggregator`, `CenterWatchCacheRegistry`,
and `CenterDb` (async best-effort). Session-ID matching prevents a stale
heartbeat task from clobbering a reconnected session.

## Peer Authentication

Full design: `docs/superpowers/specs/2026-05-20-center-fed-peer-auth-design.md`.

### Transport fail-close rules

Federation is **mTLS-only**. There is no plaintext opt-out on either end.
The Center federation gRPC server enforces the following rules at startup:

| Transport state | Outcome |
|-----------------|---------|
| No TLS block configured | Server **refuses to start** (`run()` returns `Err` → process exits) |
| TLS configured with `skip_tls=true` | Server **refuses to start** (`skip_tls` is refused on the federation path) |
| mTLS configured, `peer_identity.trust_domain` missing | Server **refuses to start** |
| Config file present but fails to parse | Server **refuses to start** (no silent default fallback) |
| mTLS configured, `peer_identity.trust_domain` present | Full SPIFFE URI-SAN verification active |

`CenterConfig` requires `peer_identity.trust_domain` under mTLS. The `allow_plaintext`
field was removed — there is no plaintext escape hatch.

```yaml
peer_identity:
  trust_domain: "edgion.io"     # required when mTLS is active
```

### SPIFFE URI-SAN binding (mTLS only)

When mTLS is active, the server verifies the client certificate on every `RegisterRequest`:

1. The certificate must carry **exactly one** URI SAN. Zero or multiple SANs → `permission_denied`.
2. The SAN must be a valid `spiffe://` URI. Parse failure → `permission_denied`.
3. The SPIFFE path must contain **exactly three segments**: `controllers/{cluster}/{name}`. Any other segment count → `permission_denied`.
4. The SPIFFE host must match `peer_identity.trust_domain` (case-insensitive). Mismatch → `permission_denied`.
5. The `{cluster}` segment must equal the `RegisterRequest.cluster` field. Mismatch → `permission_denied`.
6. The derived `controller_id` (`"{cluster}/{name}"`) must equal `RegisterRequest.controller_id`. Mismatch → `permission_denied`.

No path normalization is performed (percent-decoding, dot-segment removal, etc.). The comparison is byte-exact after case-folding only the host component.

Metric: `edgion_fed_peer_identity_check_total{result}` where `result` ∈ `ok` / `mismatch` / `no_spiffe_san` / `multi_san` / `parse_error`.

### Race-safe session takeover

A new connection for a `controller_id` that already has an active session atomically displaces the old session. The old session's teardown task cannot mark the new session offline or write stale data — session-ID matching in `mark_offline_all` ensures this. Metric: `edgion_fed_session_takeover_total` (Counter, no labels).

### Controller self-check (startup, degrade-not-crash)

On startup the Controller reads its own mTLS client certificate and verifies that the embedded SPIFFE URI SAN matches its configured `cluster` and `name`. On mismatch:

- A `tracing::error!` is emitted.
- `edgion_system_error_state{component=fed_sync_client, reason=peer_identity_self_check}` is set.
- Federation sync is **not started** for this controller (federation is auxiliary; the controller does not crash).

### Migration (breaking change)

Existing **plaintext** federation deployments must configure mTLS — there is no
plaintext opt-out. The only supported path is:

1. Configure mTLS + set `peer_identity.trust_domain` in the Center config.
2. Reissue Controller client certificates to carry exactly one URI SAN of the form
   `spiffe://{trust_domain}/controllers/{cluster}/{name}`.

A Center without valid federation mTLS (or with `skip_tls=true`) refuses to start
and the process exits non-zero. A Controller without valid federation mTLS degrades:
it logs an error, sets `system_error_state{component=fed_sync_client,
reason=no_tls_configured|skip_tls}`, and does not federate (the data plane keeps running).

See `docs/superpowers/specs/2026-05-20-center-fed-peer-auth-design.md` and
`docs/superpowers/specs/2026-05-20-fed-mtls-strict-enforce-design.md` for the full
migration guide and certificate issuance details.
