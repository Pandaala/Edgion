---
name: controller-config
description: Full schema for the edgion-controller YAML configuration.
---

# Controller YAML Configuration Schema

> Default file path: `config/edgion-controller.yaml`, specified via `--config-file`.

## Files to Reference

- Sample configuration: `config/edgion-controller.yaml`
- Configuration structure: `edgion-controller/src/config/mod.rs`
- `conf_center` top level: `edgion-controller/src/conf_mgr/conf_center/config.rs`
- FileSystem mode: `edgion-controller/src/conf_mgr/conf_center/file_system/config.rs`
- Kubernetes mode: `edgion-controller/src/conf_mgr/conf_center/kubernetes/config.rs`

## Full Schema

```yaml
# Working directory (all relative paths are resolved against this directory)
# Priority: CLI --work-dir > ENV EDGION_WORK_DIR > this value > "."
work_dir: "."

# Local admin token (Istio-style pre-shared bearer). Grants allow-all access to
# the Admin API. EDGION_ADMIN_TOKEN env var takes precedence; blank = not configured.
# Prefer the env var (from a K8s Secret) over an in-file value.
# admin_token: "edg_…"

server:
  grpc_listen: "0.0.0.0:12151"    # gRPC listen address (ConfigSyncServer)
  admin_listen: "0.0.0.0:12101"   # Admin HTTP API address (/api/v1, /configclient)
  # probe_listen: "0.0.0.0:12100"   # Liveness/readiness (/health, /ready); default 0.0.0.0:12100
  # metrics_listen: "0.0.0.0:12190" # Prometheus metrics (/metrics); default 0.0.0.0:12190
  # allow_admin_ips:                 # Optional IP allowlist for the Admin API listener; unset/empty = allow all
  #   - "10.0.0.0/8"
  #   - "127.0.0.1/32"
  #   - "::1/128"
  # allow_tcp_ips:                   # Optional L4 pre-filter (Center HTTPS only); drops non-matching peers before TLS handshake; unset/empty = allow all

logging:
  log_dir: logs                    # Log directory
  log_prefix: edgion-controller    # Log file prefix
  log_level: info                  # trace | debug | info | warn | error
                                   # Module-level filtering supported: info,edgion::core::controller=debug
  json_format: false               # JSON structured log
  console: true                    # Also output to console
  buffer_size: 10000               # Log buffer size

debug:
  enabled: true                    # Enable debug features

validation:
  enable_reference_grant_validation: true   # Enable cross-namespace ReferenceGrant validation

conf_sync:
  default_capacity: 200            # EventStore default capacity (for all resource types)
  # Override capacity for specific resource types
  # Note: setting no_sync_kinds completely replaces the default list, it is not appended
  capacity_overrides:
    # HTTPRoute: 500
    # Service: 1000
  # no_sync_kinds: [ReferenceGrant, Secret]   # Resources not synced to Gateway

# ─── gRPC channel security (optional mTLS) ───
# conf_sync_security:
#   tls:
#     cert: certs/grpc/server.crt    # Controller server certificate
#     key: certs/grpc/server.key     # Server private key
#     ca: certs/grpc/ca.crt          # CA used to verify Gateway client
#     skip_tls: false                # true = emergency downgrade to plaintext (will log ERROR)

# ─── FileSystem mode ───
conf_center:
  type: filesystem
  conf_dir: config/resources              # YAML resource directory

# ─── Kubernetes mode ───
# conf_center:
#   type: kubernetes
#   gateway_class: edgion                 # Required: GatewayClass to match
#   controller_name: edgion.io/gateway-controller  # controllerName reported in Gateway API status
#   watch_namespaces: []                  # Empty = all namespaces
#   label_selector: ""                    # Label filter
#   gateway_address: ""                   # Address reported in Gateway status
#   ha_mode: leader-only                  # "leader-only" | "all-serve"
#   no_watch_kinds: [TCPRoute, UDPRoute]  # Resource kinds to skip CRD precheck and watch for
#   leader_election:
#     lease_name: edgion-controller-leader
#     lease_namespace: edgion-system
#     lease_duration_secs: 15
#     renew_deadline_secs: 10
#     retry_period_secs: 2
#   metadata_filter:
#     label_selector: ""                  # Resource label filter
#     field_selector: ""                  # Resource field filter
#     blocked_annotations: []             # Annotations removed before entering memory (defaults remove common large kubectl/helm annotations)
#     remove_managed_fields: true         # Reduce object size
```

## Field Details

### [server]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `grpc_listen` | `String` | `0.0.0.0:12151` | ConfigSyncServer gRPC address |
| `admin_listen` | `String` | `0.0.0.0:12101` | Admin HTTP API address — serves `/api/v1/*` and `/configclient` |
| `probe_listen` | `String` | `0.0.0.0:12100` | Probe listener — serves `/health` (liveness) and `/ready` (readiness) |
| `metrics_listen` | `String` | `0.0.0.0:12190` | Metrics listener — serves `/metrics` (Prometheus) |
| `allow_admin_ips` | `Vec<String>` | `[]` (allow all) | CIDR allowlist for the Admin API listener. **Unset or empty = allow all** (backward compatible). Matched against the **TCP peer address only**; `X-Forwarded-For` is never trusted, so it is meaningful only for direct connections to the admin socket (behind an L7 proxy it degrades to matching the proxy IP). Probe/metrics run on dedicated ports and are unaffected. Dual-stack: when the listener binds `::`, list CIDRs for both families; NAT64 `64:ff9b::/96` embedded IPv4 is not extracted. Example: `["10.0.0.0/8", "127.0.0.1/32", "::1/128"]` |
| `allow_tcp_ips` | `Vec<String>` | `[]` (allow all) | **Center HTTPS only.** L4 pre-filter CIDR allowlist: peers not matching are dropped **before the rustls handshake** (saves TLS-handshake CPU, mitigates TLS-handshake DoS). **Independent** from `allow_admin_ips` (L7) — separate list. **Unset or empty = allow all** (backward compatible). Matched against the **TCP peer address only**. **Precondition:** correct only when the kernel TCP peer is the real client — behind an SNAT load balancer / terminating proxy `peer_addr()` is the LB IP, so the filter either no-ops (LB allowlisted) or drops all traffic (LB not allowlisted). Recommended operator invariant: keep `allow_tcp_ips` ⊇ `allow_admin_ips` (a stricter L4 silently drops L7-allowed IPs with no 403). Ignored when `admin_tls` is unset (plaintext) — a startup WARN is logged. Observability: the `edgion_l4_tcp_denied_total` counter on the metrics listener. |

### [logging]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `log_dir` | `String` | `logs` | Log directory (relative to work_dir) |
| `log_prefix` | `String` | `edgion-controller` | Log file prefix |
| `log_level` | `String` | `info` | Log level, supports module-level filtering |
| `json_format` | `bool` | `false` | JSON structured output |
| `console` | `bool` | `true` | Also output to console |
| `buffer_size` | `usize` | `10000` | Async log buffer size |

> Note: `log_dir` is resolved via `work_dir().resolve()`, so a relative path typically lands in `work_dir/logs/...`.

### [validation]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enable_reference_grant_validation` | `bool` | `true` | Enable ReferenceGrant validation; disabling lifts cross-namespace reference restrictions |

### [conf_sync]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `default_capacity` | `u32` | `200` | EventStore default capacity |
| `capacity_overrides` | `Map<String, u32>` | `{}` | Per-kind capacity override |
| `no_sync_kinds` | `Vec<String>?` | `["ReferenceGrant", "Secret"]` | List of resources not synced to Gateway (when set, completely replaces the default) |

### [conf_sync_security.tls]

Optional configuration; when enabled, the Controller's gRPC port uses mTLS.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `cert` | `String` | — | Server certificate path (PEM, relative to work_dir) |
| `key` | `String` | — | Server private key path (PEM) |
| `ca` | `String` | — | CA certificate path (used to verify the Gateway client certificate) |
| `skip_tls` | `bool` | `false` | Emergency downgrade switch: `true` skips TLS and starts in plaintext (logs ERROR) |

> When this section is not configured, gRPC runs in plaintext, fully compatible with previous versions.
> Generate certificates with `edgion-cli gen-certs --output file --output-dir <dir>`.

### [conf_center] — FileSystem

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `type` | `String` | — | `"filesystem"` |
| `conf_dir` | `PathBuf` | — | Resource configuration directory |

Important practical constraints:
- `conf_dir` is currently resolved by `FileSystemStorage::new()` directly against the process cwd
- That is, it does not automatically follow `work_dir` like many log paths do
- Prefer absolute paths when you want to avoid ambiguity

CLI special cases:
- `--conf-dir` directly overrides this `conf_dir`
- It also switches `conf_center` to the FileSystem variant

### [conf_center] — Kubernetes

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `type` | `String` | — | `"kubernetes"` |
| `gateway_class` | `String` | **Required** | Name of the GatewayClass to match |
| `controller_name` | `String` | — | controllerName reported in Gateway API status |
| `watch_namespaces` | `Vec<String>` | `[]` (all) | List of namespaces to watch |
| `label_selector` | `String` | `""` | Resource label filter |
| `gateway_address` | `String?` | — | Address reported in Gateway status |
| `ha_mode` | `String` | — | `"leader-only"` or `"all-serve"` |
| `no_watch_kinds` | `Vec<String>` | `[]` | Resource kinds (PascalCase) not to probe/watch, e.g., `["TCPRoute", "UDPRoute", "BackendTLSPolicy"]`. Skips CRD precheck at startup, does not spawn a watcher, and is excluded from processor readiness |

### [conf_center.leader_election]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `lease_name` | `String` | `edgion-controller-leader` | K8s Lease name |
| `lease_namespace` | `String` | `edgion-system` | K8s Lease namespace |
| `lease_duration_secs` | `u64` | `15` | Lease duration in seconds (validity on the apiserver side) |
| `renew_deadline_secs` | `u64` | `10` | Renewal deadline window: within this window after the most recent successful renewal, the Leader retains leadership and retries even when transient errors (5xx / network) occur, avoiding spurious leader flips caused by apiserver flapping. Constraint: `retry_period_secs < renew_deadline_secs < lease_duration_secs`. |
| `retry_period_secs` | `u64` | `2` | Uniform sleep interval between each acquire/renew attempt (corresponds to client-go `RetryPeriod`). Roughly `renew_deadline_secs / retry_period_secs` retries occur within a single `renew_deadline_secs` window. |

### [conf_center.metadata_filter]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `blocked_annotations` | `Vec<String>` | Common large kubectl/helm annotations | Annotations removed before entering memory |
| `remove_managed_fields` | `bool` | `true` | Reduce object size |

## `admin_token` — Admin API Authentication

The Admin API uses **token-only** authentication (Istio-style pre-shared bearer token). There is no OIDC admin, no username/password, and no httpOnly cookie. The operator supplies a single admin token that grants allow-all (wildcard `*`/`*` RBAC policy) access to the Admin API.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `admin_token` | `Option<String>` | `None` | Raw bearer token. A blank/whitespace value is treated as **not configured**. The `EDGION_ADMIN_TOKEN` env var takes precedence over this field. Maps to an allow-all wildcard policy; survives reloads. |

The token is resolved from two sources (highest precedence first):

1. **Env var** `EDGION_ADMIN_TOKEN`, injected via `valueFrom.secretKeyRef` from the `edgion-admin-credentials` Secret (key `token`; recommended for Kubernetes).
2. **`admin_token`** config field (a Secret-mounted config file).

```yaml
# Prefer the env var over an in-file value:
admin_token: "edg_…"
```

When **no** token is configured (neither `EDGION_ADMIN_TOKEN`/`admin_token` nor any `cli_tokens`), the server still boots, but all Admin API business routes return `503` fail-close with a startup WARN. `/health` / `/ready` / `/metrics` / `/api/v1/auth/status` remain accessible. Scoped, role-limited tokens are configured separately under [`[cli_tokens]`](#cli_tokens--static-cli-tokens-admin-api-access).

See `skills/02-features/02-config/03-auth-bootstrap.md` for the full matrix and migration guide.

## `[center]` — Federation Client

Connects this Controller to a Federation Center. Omit this section entirely if you are not using federation.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `address` | `String` | **Required** | gRPC address of the Center server (e.g. `https://center.example.com:12251`) |
| `name` | `String` | **Required** | Logical name of this Controller, reported to Center |
| `cluster` | `String` | **Required** | Cluster name, used for routing and aggregation |
| `enabled` | `bool` | `true` | Hot-reloadable kill-switch. `false` stops the federation client without a restart. Reloaded via `POST /api/v1/reload`. |
| `env` | `Vec<String>` | `[]` | Environment tags reported to Center (e.g. `["production"]`). |
| `tag` | `Vec<String>` | `[]` | Arbitrary extension tags reported to Center. |
| `ping_interval_secs` | `u64` | `30` | Heartbeat interval in seconds for the federation keep-alive ping. |
| `security` | `ConfSyncSecurityConfig` | — | mTLS certificates for the gRPC channel (same format as `conf_sync_security`). Required -- federation fails closed without it. |
| `rbac` | `FedRbacConfig?` | `null` | RBAC for Center operations. **Absent => built-in default policy applies (read `get`/`list`/`watch`/`list-keys` on all ResourceKinds except `Secret`; write `create`/`update`/`delete` on `PluginMetaData`; `get`/`list`/`failover` on the synthetic `RegionRoute`). Present => the explicit rules fully override the default.** |

### `[center.rbac]` -- Federation RBAC

**When `rbac` is absent** (but `[center]` is present), a built-in default policy applies:

| What | Allowed |
|------|---------|
| Read (`get`, `list`, `watch`, `list-keys`) | All resource kinds **except** `Secret` (EdgionTls IS readable; only the Secret kind is excluded for credential safety) |
| Write (`create`, `update`, `delete`) | `PluginMetaData` only (drives IP-restriction sync + region-route sync; grants write to ALL plugin metadata) |
| `get`, `list`, `failover` | `RegionRoute` (synthetic label): get/list allow the aggregate read endpoints; failover covers the POST failover endpoint |
| Everything else (`reload`, `server-info`, `diagnostics`, etc.) | Denied |

**When `rbac` is present**, the explicit rules fully override the built-in default. This is how operators grant more (e.g., Secret read or reload) or less than the default.

> **Security:** There is no server-side Secret backstop on the federation path. A wildcard read (`kinds: ["*"]` with `get`, `list`, or `watch`) lets Center read Secret / EdgionTls private-key material via the blind proxy. Always enumerate concrete kinds for any UI surface. The built-in default avoids this by excluding `Secret` explicitly.

**Example: explicit minimal allowlist (automated operations only):**

```yaml
center:
  address: "https://center.example.com:12251"
  name: "ctrl-1"
  cluster: "prod"
  enabled: true
  security:
    active: primary
    certs:
      - name: primary
        cert: certs/grpc/primary/client.crt
        key: certs/grpc/primary/client.key
        ca: certs/grpc/primary/ca.crt
  rbac:
    allow:
      - verbs: [get, list, watch, create, update, delete]
        kinds: [PluginMetaData]
      - verbs: [failover]
        kinds: [RegionRoute]
      - verbs: [reload]
        kinds: ["*"]
```

| Rule | Covers |
|------|--------|
| `get/list/watch/create/update/delete` on `PluginMetaData` | IP-restriction plugin sync + region-route sync; `list` also serves Center blind-proxy UI list pages |
| `failover` on `RegionRoute` | Regional failover endpoint |
| `reload` on `*` | Config reload (non-resource, wildcard kind is safe for write verbs) |

> **Note on `kinds: ["*"]` with `reload`:** using `*` is safe here because `reload` is a write-only action with no read surface. The wildcard-read prohibition applies to `get`, `list`, and `watch` only.

> **Cross-reference:** this change also fixes review finding `controller-01` -- the federation path now routes through the Admin API authorization layer instead of a bare unauthenticated router. See `skills/04-review/architecture/fed-proxy-mtls-fail-close.md` for the prior mTLS fix and the new RBAC fix status.

> **Full operator guide:** see `docs/en/ops-guide/federation-rbac.md` for authorization examples, security notes, and the kill-switch reference.

`FedAllowRule.verbs` accepts: `get`, `list`, `list-keys`, `watch`, `create`, `update`, `delete`, `failover`, `acme-trigger`, `reload`, `server-info`, `diagnostics`, `wipe-all`, `debug`.

> **Kind casing:** `kinds` MUST use the canonical PascalCase resource Kind name (e.g. `PluginMetaData`, `HTTPRoute`, `RegionRoute`). Lowercase or alias spellings (e.g. `pluginmetadata`) are matched case-sensitively against the normalized request kind and will silently never match. Verbs are case-insensitive.

## `[cli_tokens]` — Static CLI Tokens (Admin API Access)

Static Bearer tokens that grant role-scoped access to the Controller Admin API. Each entry binds a token to a fine-grained RBAC policy using the same `(verb, kind)` model as `[center].rbac`. Tokens are never stored in plaintext — only their SHA-256 hex digest is persisted. Hot-reloadable.

**Source:** `edgion-controller/src/config/mod.rs` (`CliTokenConfig`).

### Field Reference

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | `String` | **Required** | Human-readable label used in audit/log fields. Must match `[A-Za-z0-9_.-]+`. |
| `token_sha256` | `String` | **Required** | SHA-256 hex digest (64 lowercase hex chars) of the raw bearer token. Never store the token itself. Mint via `edgion-cli gen-token`. |
| `rbac` | `FedRbacConfig` | deny-all | RBAC allow-rules for this token. Absent or empty ⇒ deny-all. Same `verbs × kinds` schema as `[center].rbac`. |

### Config Example

```yaml
cli_tokens:
  # Read-only token — safe for monitoring/CI pipelines that only inspect routes
  - name: monitor-bot
    # Mint the token with: edgion-cli gen-token
    # Paste the SHA-256 line here; distribute the edg_… token to the consumer.
    token_sha256: "0000000000000000000000000000000000000000000000000000000000000000"
    rbac:
      allow:
        # Enumerate concrete kinds; NEVER use kinds: ["*"] with read verbs
        # (wildcard read exposes Secret and EdgionTls private-key material).
        - verbs: [get, list]
          kinds: [HTTPRoute, GRPCRoute, TCPRoute, Service, EdgionGatewayConfig, EdgionTls]

  # Scoped read-write token — manages plugin metadata and can trigger reload
  - name: plugin-manager
    token_sha256: "1111111111111111111111111111111111111111111111111111111111111111"
    rbac:
      allow:
        - verbs: [get, list, watch, create, update, delete]
          kinds: [PluginMetaData]
        # reload is a write-only verb; kinds: ["*"] is safe here (no read surface)
        - verbs: [reload]
          kinds: ["*"]
```

> **Kind casing:** `kinds` MUST use the canonical PascalCase resource Kind name (e.g. `PluginMetaData`, `HTTPRoute`). Lowercase spellings silently never match.
>
> **Full operator guide:** see `docs/en/ops-guide/federation-rbac.md#cli-token-rbac` for the gen-token workflow, security notes, and rotation instructions.

## `path_normalization` — Path Normalization (Gateway-side configuration)

Path normalization configuration is read on the Gateway side (`edgion-resources/src/path_normalization/config.rs`); the Controller config does not directly own this section, but skills documentation records it here for unified reference.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `reject_escaped_slashes` | `bool` | `false` | Reject requests containing `%2F` / `%5C` (high-security scenarios) |
| `reject_invalid_percent_encoding` | `bool` | `false` | Reject requests containing invalid percent-encoding (overlong UTF-8, etc.) |
| `forward_normalized_path` | `bool` | `true` | (v4) Forward the normalized path to the upstream by default. When set to false, all routes default to forwarding raw (compatible with v3.1 behavior, soft-fallback channel). For per-route override see annotation `edgion.io/forward-raw-path`. |
| `preserve_path_parameters` | `bool` | `false` | (v4) Preserve path-parameter literals (`;param=...`). Default false (stripped, consistent with v3.1, preserving the §1.4 path-parameter bypass guarantee). When set to true, `..;/` -style bypasses move out of the default guarantee scope (see spec §14.4 for details). |

## Most Common Controller Configuration Changes in This Project

- Switch the configuration directory for local testing: change `conf_center.type = "file_system"` + `conf_dir`
- Limit a K8s deployment to a single GatewayClass: change `gateway_class`
- Control cache caps for different kinds: change `conf_sync.capacity_overrides`
- Tune observability noise: change `logging.log_level`
