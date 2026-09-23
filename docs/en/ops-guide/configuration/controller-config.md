# Controller Configuration

edgion-controller reads its configuration from a YAML file. The default path is `config/edgion-controller.yaml`; use `--config-file` to specify a different location.

## Full Example

```yaml
# Working directory — all relative paths are resolved against this
# Priority: CLI --work-dir > env EDGION_WORK_DIR > this value > "."
work_dir: "."

server:
  grpc_listen: "0.0.0.0:12151"   # ConfigSyncServer gRPC listen address
  admin_listen: "127.0.0.1:12101"  # Admin HTTP API address (/api/v1, /configclient) — loopback default; set 0.0.0.0 + allow_admin_ips for cluster/remote access
  # probe_listen: "0.0.0.0:12100"   # Liveness/readiness (/health, /ready); default 0.0.0.0:12100
  # metrics_listen: "0.0.0.0:12190" # Prometheus metrics (/metrics); default 0.0.0.0:12190

logging:
  log_dir: logs                   # System log directory (relative to work_dir)
  log_prefix: edgion-controller   # Log file name prefix
  log_level: info                 # trace | debug | info | warn | error
                                  # Module-level supported: info,edgion::core::controller=debug
  json_format: false              # Output logs in JSON format
  console: true                   # Also print to stdout
  buffer_size: 10000              # Async log buffer size

debug:
  enabled: true                   # Enable debug features

validation:
  enable_reference_grant_validation: true  # Enforce ReferenceGrant cross-namespace validation

conf_sync:
  default_capacity: 200           # Default EventStore capacity for all resource kinds
  capacity_overrides:
    # Override capacity per resource kind, e.g.:
    # HTTPRoute: 500
    # Service: 1000

  # no_sync_kinds: [ReferenceGrant, Secret]
  # Kinds NOT synced to Gateway — setting this REPLACES the default list entirely, not appended

# ─── gRPC channel security (REQUIRED) ─────────────────────────────
# Omitting this section makes the Controller REFUSE TO START. Pick exactly one of
# the three options below (auto_ca / cert paths / skip_tls).
#
# Option 1 — Kubernetes: the Controller's internal CA provisions everything.
conf_sync_security:
  auto_ca: true
#
# Option 2 — file-mode mTLS:
# conf_sync_security:
#   tls:
#     cert: certs/grpc/server.crt   # Controller server certificate (PEM)
#     key: certs/grpc/server.key    # Server private key (PEM)
#     ca: certs/grpc/ca.crt         # CA to verify Gateway client certificates
#
# Option 3 — explicit plaintext opt-in (local dev / non-production only, logs ERROR):
# conf_sync_security:
#   tls:
#     skip_tls: true

# ─── conf_center: FileSystem mode (local development) ─────────────
conf_center:
  type: file_system
  controller_name: edgion.io/gateway-controller
  conf_dir: config/resources      # YAML resource directory (prefer absolute paths)

# ─── conf_center: Kubernetes mode (production) ────────────────────
# conf_center:
#   type: kubernetes
#   controller_name: edgion.io/gateway-controller  # controllerName reported in Gateway API status
#   watch_namespaces: []                           # Empty = watch all namespaces
#   label_selector: ""                             # Label filter for watched resources
#   gateway_address: ""                            # Address reported in Gateway status
#   ha_mode: leader-only                           # "leader-only" | "all-serve"
#   no_watch_kinds: [TCPRoute, UDPRoute]           # Skip CRD pre-check and watch for these kinds
#   cache_ready_deadline_secs: 90                  # Pre-CachesReady deadline (secs); restart-bound, not hot-reloadable
#   leader_election:
#     lease_name: edgion-controller-leader
#     lease_namespace: edgion-system
#     lease_duration_secs: 15
#     renew_deadline_secs: 10
#     retry_period_secs: 2
#   metadata_filter:
#     label_selector: ""          # Resource label filter
#     field_selector: ""          # Resource field filter
#     blocked_annotations: []     # Annotations stripped before entering memory (default: common kubectl/helm annotations)
#     remove_managed_fields: true # Strip managedFields to reduce object size
```

## Configuration Parameters

### [server]

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `grpc_listen` | `String` | `0.0.0.0:12151` | gRPC listen address for ConfigSyncServer; Gateways connect here to receive config |
| `admin_listen` | `String` | `127.0.0.1:12101` | Admin HTTP API listen address — serves `/api/v1/*` and `/configclient`. Defaults to loopback; set `0.0.0.0` and configure `allow_admin_ips` for cluster or remote access |
| `probe_listen` | `String` | `0.0.0.0:12100` | Probe listener — serves `/health` (liveness) and `/ready` (readiness); use `httpGet.port: 12100` in Kubernetes pod spec |
| `metrics_listen` | `String` | `0.0.0.0:12190` | Metrics listener — serves `/metrics` (Prometheus scrape target) |

### [logging]

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `log_dir` | `String` | `logs` | Log directory, resolved relative to `work_dir` |
| `log_prefix` | `String` | `edgion-controller` | Log file name prefix |
| `log_level` | `String` | `info` | Log level: `trace`, `debug`, `info`, `warn`, `error`. Supports module-level filtering (e.g. `info,edgion::core::controller=debug`) |
| `json_format` | `Boolean` | `false` | Emit structured JSON logs |
| `console` | `Boolean` | `true` | Also write logs to stdout |
| `buffer_size` | `Integer` | `10000` | Async log buffer size |

### [debug]

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enabled` | `Boolean` | `true` | Enable debug features |

### [validation]

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enable_reference_grant_validation` | `Boolean` | `true` | Enforce ReferenceGrant validation for cross-namespace references. When disabled, cross-namespace references are unrestricted |

### [conf_sync]

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `default_capacity` | `Integer` | `200` | Default EventStore capacity for all resource kinds |
| `capacity_overrides` | `Map<String, Integer>` | `{}` | Per-kind capacity overrides, e.g. `HTTPRoute = 500` |
| `no_sync_kinds` | `Array<String>` | `["ReferenceGrant", "Secret"]` | Kinds NOT synced to Gateway. **Setting this replaces the default list entirely — it is not appended**. Optional |

### [conf_sync_security]

**Required.** The conf_sync gRPC channel carries resolved Secrets (TLS private keys, backend
credentials) to the data plane, so its transport must be chosen explicitly.

**Omitting the `conf_sync_security` section entirely makes the Controller refuse to start**
(non-zero exit, with an actionable error). Configure exactly one of:

| Configuration | Result |
|---------------|--------|
| `auto_ca: true` | mTLS — the Controller's internal CA provisions the CA and the Gateway client cert (recommended on Kubernetes) |
| `certs` / legacy `tls` block with `cert` + `key` + `ca` | File-mode mTLS |
| `tls.skip_tls: true` | Plaintext, explicitly opted in (logs ERROR; local dev / non-production only) |
| *(section omitted)* | **Refuses to start** |

> **Migration**: omitting the section used to mean "plaintext gRPC". Deployments that relied on
> that will now fail to start. To keep plaintext, opt in explicitly with
> `conf_sync_security: {tls: {skip_tls: true}}`; otherwise configure `auto_ca` or cert paths.

#### [conf_sync_security.tls]

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `cert` | `String` | — | Server certificate path (PEM, relative to work_dir) |
| `key` | `String` | — | Server private key path (PEM) |
| `ca` | `String` | — | CA certificate path used to verify Gateway client certificates |
| `skip_tls` | `Boolean` | `false` | Explicit plaintext opt-in: `true` skips TLS and starts in plaintext mode (logs ERROR and raises `edgion_system_error_state{reason="skip_tls"}`) |

Certificates can be generated with `edgion-cli gen-certs --output file --output-dir <dir>`.

`conf_center.type` selects the backend. Accepted values are `file_system`,
`kubernetes`, and `etcd` (snake_case — `file_system`, not `filesystem`).

### Controller identity

Set `conf_center.controller_name` to the exact `spec.controllerName` used by your
GatewayClasses. It must be a Gateway API domain-prefixed path of at most 253
characters. Blank or invalid configured values fail startup.

FileSystem and etcd require this field. In Kubernetes only, omitting it derives
`edgion.io/gateway-controller/{POD_NAMESPACE}/{EDGION_CONTROLLER_DEPLOYMENT_NAME}`.
Inject `POD_NAMESPACE` from `metadata.namespace`, and explicitly inject
`EDGION_CONTROLLER_DEPLOYMENT_NAME` with the stable Deployment name, never a Pod
name. Missing or invalid components fail startup. For namespace `edgion-system`
and Deployment `edgion-controller`, configure the GatewayClass with
`edgion.io/gateway-controller/edgion-system/edgion-controller`.

Independent deployments must use distinct controller names and GatewayClasses;
explicit identity collisions are not detected at runtime. Replicas sharing an
identity must share the configuration view and election domain.
Independent Kubernetes deployments also need separate leader-election Leases;
generating a controller name does not change lease_name or lease_namespace.
GatewayClass `spec.controllerName` is immutable; create a replacement class when
changing its identity.

The Controller serves every GatewayClass whose `spec.controllerName` matches its
identity and validates each class's parameters. Each Gateway uses its own class's
`parametersRef`; adding a matching class requires no Controller restart. There is
no class-name selector. Supplying the removed `conf_center.gateway_class` key,
including null, fails startup. Use RBAC to control class creation and controllerName
assignment. Multiple classes cannot share one physical listener port. GatewayClass watches
ignore `label_selector` to cover unlabeled classes too. Adding `GatewayClass` to
`no_watch_kinds` disables this reporting and prevents Gateway processing.

### [conf_center] — FileSystem Mode

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `type` | `String` | — | Must be `file_system` |
| `controller_name` | `String` | **Required** | Explicit Gateway API controller identity |
| `conf_dir` | `Path` | — | YAML resource directory. **Use absolute paths** — the current implementation does not resolve this relative to `work_dir` |

Note: The `--conf-dir` CLI flag overrides `conf_dir` and switches `conf_center` to FileSystem mode.

### [conf_center] — Kubernetes Mode

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `type` | `String` | — | Must be `"kubernetes"` |
| `controller_name` | `String` | Namespace + Deployment identity | Explicit override of the generated controller identity |
| `watch_namespaces` | `Array<String>` | `[]` (all) | Namespaces to watch; empty list means all namespaces |
| `label_selector` | `String` | `""` | Label selector filter for watched resources, excluding GatewayClass |
| `gateway_address` | `String` | — | Address reported in Gateway status. Optional |
| `ha_mode` | `String` | — | HA mode: `"leader-only"` or `"all-serve"` |
| `no_watch_kinds` | `Array<String>` | `[]` | Resource kinds to skip CRD pre-check and watch for (PascalCase), e.g. `["TCPRoute", "UDPRoute"]` |
| `cache_ready_deadline_secs` | `Integer` | `90` | Deadline (seconds) for a started watcher set to reach `CachesReady` — covers the internal 30 s readiness-poll budget plus the post-ready tail (WatchObj registration, revalidations, leader-job start). **Must be `> 30`** (startup validation error otherwise). **Restart-bound, not hot-reloadable** — `POST /api/v1/reload` re-arms the deadline every iteration but always with the boot-time value; changing it requires a process restart. Sizing rule of thumb: raise it when a single resource kind's count approaches the per-kind workqueue capacity (5000) — a too-small value is a *permanent* restart loop, not a one-off failure, since the pod replays from scratch on every deadline expiry. Worst case, the pre-ready wait before a restart fires is ≈2× the configured value (the same deadline budget also wraps the initial `start_event_watchers` call via a timeout, then re-arms fresh for the tail). See [Supervision & recovery](https://github.com/Pandaala/Edgion/blob/main/skills/01-architecture/01-controller/03-config-center/02-kubernetes/00-lifecycle.md#supervision--recovery) for the full mechanism |

At startup, the Controller waits for a complete initial load of its required
watch sources before publishing configuration synchronization. Later Kubernetes
relists retain the last complete source snapshot until replacement succeeds,
then reconcile that source through the existing worker. An ordinary relist does
not restart all resource controllers or withdraw the configuration-sync service.
Namespace label changes also refresh affected Selector Gateways.

Administrative reloads and terminal controller failures still rebuild resource
state after stopping the old tasks. In `leader-only` mode, leadership loss also
stops that resource generation. In `all-serve` mode, leadership changes preserve
resource watching and configuration serving while changing leader-only services.

A larger startup deadline helps finite slow processing. It cannot resolve an
actual queue-capacity deadlock; repeated initialization timeouts require
investigating the stalled processing path as well as resource volume.

### [conf_center.leader_election] — Kubernetes Mode

Kubernetes-mode leader election uses a K8s Lease object. **These fields are distinct from the
Etcd-mode `leader_election` block below** — the Etcd block has no `lease_name`/`lease_namespace`
(there is no Lease object) and instead requires `grpc_addr`/`admin_addr`.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `lease_name` | `String` | `edgion-controller-leader` | Kubernetes Lease resource name |
| `lease_namespace` | `String` | `edgion-system` | Namespace where the Lease lives |
| `lease_duration_secs` | `Integer` | `15` | Lease hold duration in seconds (server-side validity on the apiserver) |
| `renew_deadline_secs` | `Integer` | `10` | Renewal deadline window: the leader keeps leadership across transient errors as long as the last successful renew was within this window. Constraint: `retry_period_secs < renew_deadline_secs < lease_duration_secs` |
| `retry_period_secs` | `Integer` | `2` | Sleep interval between each acquire/renew attempt (client-go `RetryPeriod`) |

### [conf_center] — Etcd Mode

Etcd mode targets non-Kubernetes (bare-metal / VM) production, where the Controller stores and watches
resources in an etcd cluster instead of the Kubernetes API. It supports active-passive HA through the
`leader_election` block below.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `type` | `String` | — | Must be `etcd` |
| `controller_name` | `String` | **Required** | Explicit Gateway API controller identity |
| `endpoints` | `Array<String>` | **Required** | etcd cluster endpoints, e.g. `["http://10.0.0.1:2379"]` |
| `key_prefix` | `String` | `/edgion` | Prefix for all configuration keys stored in etcd |
| `auth` | Object | — | Optional etcd username/password authentication (`auth.username`, `auth.password`) |
| `tls` | Object | — | Optional TLS for the **Controller→etcd** connection (file paths): `tls.ca_cert_file`, `tls.client_cert_file`, `tls.client_key_file`. This is unrelated to the conf_sync mTLS channel |
| `connect_timeout_ms` | `Integer` | `5000` | etcd connection (dial) timeout in milliseconds |
| `request_timeout_ms` | `Integer` | `10000` | Per-request timeout for the shared conf_center etcd client's unary calls, in milliseconds. **Note**: this is NOT applied to the blocking leader-election `campaign()` RPC (which the election client runs without a channel-wide timeout on purpose), so operators do not need to tune it for election |
| `leader_election` | Object | — | Active-passive HA settings (see below). **Always on**; there is no enable/disable toggle |

### [conf_center.leader_election] — Etcd Mode

Etcd-mode leader election uses etcd's official `v3election` gRPC service plus a self-managed lease
keepalive. **These fields are DISTINCT from the Kubernetes `leader_election` block above** — the Etcd
block has **no** `lease_name`/`lease_namespace` (there is no K8s Lease object) and instead **requires**
`grpc_addr` and `admin_addr`.

Exactly one Controller becomes leader (runs the resource controllers and serves conf_sync); standbys
reject data-plane traffic and, on the Admin API, return `409` naming the leader's `admin_addr`. On a
single node the sole candidate wins the election instantly. Kubernetes mode (which has its own
Lease-based HA) is unaffected.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `identity` | `String` | hostname | Human-facing label only — used in logs and the not-leader `409` body. **Never** used for an ownership decision (per-lease keys make incarnations distinct). Defaults to the host's `HOSTNAME` |
| `grpc_addr` | `String` | **Required** | The conf_sync gRPC address gateways connect to. Must be a **routable, non-loopback** address (not `127.0.0.1` / `0.0.0.0` / `localhost`) — it is the failover target gateways rotate to |
| `admin_addr` | `String` | **Required** | The Admin HTTP address named in the not-leader `409` body. Must be a **routable, non-loopback** address |
| `lease_duration_secs` | `Integer` | `15` | Session lease TTL. The double-leader window and teardown budget are both bounded by `lease_duration − renew_deadline` |
| `renew_deadline_secs` | `Integer` | `10` | The leader self-demotes if it cannot record a successful keepalive within this window |
| `retry_period_secs` | `Integer` | `2` | Keepalive / guard-txn cadence |

**Startup validation (fail fast).** The Controller rejects the config at startup unless
`retry_period_secs < renew_deadline_secs < lease_duration_secs`, all `> 0`, **and**
`floor(renew_deadline_secs / retry_period_secs) >= 3`. The defaults `2 / 10 / 15` satisfy this.

**Deployment requirements and caveats:**

- **`v3election` must be available.** Stock etcd serves it, but some proxied/managed etcd deployments
  (and certain `grpc-proxy` configurations) disable the `v3election`/`v3lock` gRPC services. The
  Controller probes it at startup and fails fast with an actionable error if it is absent.
- **These defaults assume a LAN-local etcd.** A WAN-separated etcd should raise all three values, and
  etcd's own election timeout must be kept well under `renew_deadline_secs` (validation cannot check
  this).
- **conf_sync mTLS requires manually-issued certs in etcd mode.** The `auto_ca` automatic cert
  provisioning is **Kubernetes-only** (it needs a kube API client and hard-codes K8s Service DNS SANs)
  and fails at startup on bare-metal/VM. Use `edgion-cli gen-certs --server-san <name>` (repeatable) and
  ensure the conf_sync **server** certificate's SAN covers **every** Controller's `grpc_addr` a gateway
  may rotate to across failover — otherwise the gateway fails TLS verification when it reconnects to a
  new leader.
- **A standby's `/ready` returns `503`** (it has no ConfigSyncServer). Do **not** front the Admin/probe
  port with a TCP/HTTP liveness check that would kill a healthy standby; health and metrics endpoints
  remain reachable.

### [conf_center.metadata_filter]

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `label_selector` | `String` | `""` | Label selector applied to watched resources |
| `field_selector` | `String` | `""` | Field selector applied to watched resources |
| `blocked_annotations` | `Array<String>` | Common kubectl/helm annotations | Annotations stripped from resources before they enter memory |
| `remove_managed_fields` | `Boolean` | `true` | Strip `managedFields` to reduce object size |

## Common Scenarios

### Local Development with FileSystem Mode

Use local YAML files to simulate Kubernetes resources without a cluster:

```yaml
conf_center:
  type: file_system
  controller_name: edgion.io/gateway-controller
  conf_dir: /home/user/my-project/config/resources
```

You can also override at startup via CLI:

```bash
./edgion-controller --config-file config/edgion-controller.yaml \
  --conf-dir /home/user/my-project/config/resources
```

### Kubernetes Production Deployment

```yaml
conf_center:
  type: kubernetes
  controller_name: edgion.io/gateway-controller
  watch_namespaces: [default, production]
  gateway_address: "203.0.113.10"
  ha_mode: leader-only
```

### Adjusting Log Verbosity

Temporarily increase detail for a specific module to debug issues:

```yaml
logging:
  log_level: "info,edgion::core::controller=debug"
  json_format: true  # Easier to search with structured queries
```

### HA Multi-Replica Deployment

Enable leader election for multi-replica Kubernetes deployments:

```yaml
conf_center:
  type: kubernetes
  ha_mode: leader-only
  controller_name: edgion.io/gateway-controller
  leader_election:
    lease_name: edgion-controller-leader
    lease_namespace: edgion-system
    lease_duration_secs: 15
    renew_deadline_secs: 10
    retry_period_secs: 2
```

### Enabling mTLS

Encrypt gRPC communication between Controller and Gateway:

```yaml
conf_sync_security:
  tls:
    cert: certs/grpc/server.crt
    key: certs/grpc/server.key
    ca: certs/grpc/ca.crt
```

### Non-Kubernetes (VM / bare-metal) HA Deployment

Run two (or more) Controllers against a shared etcd cluster for active-passive HA. Leader election is
**always on** — exactly one Controller becomes leader; the rest stand by and take over on failure.
Gateways list every Controller's `grpc_addr` and rotate to whichever is currently the leader.

Controller **A** (`10.0.0.11`):

```yaml
conf_center:
  type: etcd
  controller_name: edgion.io/gateway-controller
  endpoints: ["http://10.0.0.1:2379", "http://10.0.0.2:2379", "http://10.0.0.3:2379"]
  key_prefix: /edgion
  leader_election:
    identity: controller-a          # optional; human-facing (logs + 409 body) only
    grpc_addr: 10.0.0.11:12151      # routable; the conf_sync gRPC port gateways rotate to
    admin_addr: 10.0.0.11:12101     # routable; named in the not-leader 409 body
    lease_duration_secs: 15
    renew_deadline_secs: 10
    retry_period_secs: 2
```

Controller **B** (`10.0.0.12`) is identical except for the addresses:

```yaml
  leader_election:
    identity: controller-b
    grpc_addr: 10.0.0.12:12151
    admin_addr: 10.0.0.12:12101
```

Each gateway lists both Controllers and rotates to the current leader (a one-element list keeps
today's single-Controller behavior):

```yaml
# edgion-gateway.yaml
gateway:
  server_addrs:
    - "https://10.0.0.11:12151"
    - "https://10.0.0.12:12151"
```

**Caveats (see [conf_center.leader_election] — Etcd Mode above for detail):**

- **`v3election` is required.** The Controller probes it at startup and fails fast if the managed/proxied
  etcd has it disabled.
- **conf_sync mTLS needs manually-issued certs** — `auto_ca` is Kubernetes-only. The **server**
  certificate's SAN must cover **every** Controller's `grpc_addr` (here both `10.0.0.11` and
  `10.0.0.12`), or a gateway fails TLS verification after failover:

  ```bash
  edgion-cli gen-certs --output file --output-dir /usr/local/edgion/certs/grpc \
      --server-san 10.0.0.11 --server-san 10.0.0.12
  ```

- **A standby's `/ready` returns `503`.** Do not use the Admin `/ready` as a plain liveness check that
  would restart a healthy standby.

## Authentication

The Controller Admin API uses **Istio-style, token-only** authentication. Every request to a `/api/v1/*` business route must carry an `Authorization: Bearer <token>` header. There is **no** browser login, **no** session cookie, and **no** OIDC for the Admin API — those were removed. (The browser dashboard now lives in the separate Center project; the Controller no longer hosts one.)

A single pre-shared **admin token** grants full (allow-all) access. It is supplied via one of the following (highest precedence first):

1. Env var `EDGION_ADMIN_TOKEN`, injected via `valueFrom.secretKeyRef` from the `edgion-admin-credentials` Secret (key `token`) — recommended for Kubernetes. `deploy.sh` creates this Secret idempotently.
2. The top-level `admin_token` config field (see below).

A blank or absent token means "not configured". When no admin token is set **and** no `cli_tokens` are configured, the server still boots, but all Admin API business routes return **503 fail-close**. `/health`, `/ready`, `/metrics`, and `GET /api/v1/auth/status` remain accessible.

Retrieve the token from the Secret (if using `deploy.sh`):

```bash
kubectl -n edgion-system get secret edgion-admin-credentials \
  -o jsonpath='{.data.token}' | base64 -d
```

Authorize the CLI with it:

```bash
edgion-cli login --token <TOKEN>
```

### Authorization model

A token maps to an RBAC policy. The **local admin token** maps to an allow-all (wildcard) policy — it can perform every operation on every kind. Fine-grained, per-token RBAC is available through `cli_tokens` (see below) and through federation (`center.rbac`); both are unchanged. There is no separate "superuser" concept beyond the allow-all admin token.

Authentication is **Bearer-only** — there is no httpOnly cookie and no server-side session.

### Auth endpoint

`GET /api/v1/auth/status` is the only auth endpoint and is always public (no token required). It returns:

```json
{ "auth_required": true, "mode": "token", "status": "..." }
```

The former `/api/v1/auth/login`, `/api/v1/auth/logout`, and `/api/v1/auth/me` endpoints have been removed.

### `admin_token`

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `admin_token` | `String` | _(empty)_ | Pre-shared admin bearer token granting allow-all access. Blank = not configured. Prefer injecting via the `EDGION_ADMIN_TOKEN` env var from a Secret over writing it as plaintext in the config file. |

```yaml
# Top-level config field. Prefer EDGION_ADMIN_TOKEN env injection from a Secret
# over committing the token to a plaintext config file.
admin_token: "edg_..."
```

> **Note**: Center keeps its own auth bootstrap (OIDC / `local_auth`) and is unaffected by this token-only model; only the Controller Admin API changed. See the [Federation Client](#federation-client-center) section for joining a Center.

### [cli_tokens] — Static CLI Tokens

Static Bearer tokens for programmatic or CI access to the Admin API. Each token is bound to a fine-grained RBAC policy. Tokens are stored as SHA-256 hex digests (never plaintext). Hot-reloadable.

| Parameter | Type | Description |
|-----------|------|-------------|
| `name` | `String` | Human-readable label for this token |
| `token_sha256` | `String` | SHA-256 hex digest (64 lowercase hex chars) of the raw bearer token. Generate with `edgion-cli gen-token` |
| `rbac` | Object | RBAC allow-rules (`verbs × kinds`). Absent or empty → deny-all |

```yaml
cli_tokens:
  - name: "ci-readonly"
    token_sha256: "0000000000000000000000000000000000000000000000000000000000000000"
    rbac:
      allow:
        # Enumerate concrete kinds; never use kinds: ["*"] with read verbs
        # (a wildcard read exposes Secret and EdgionTls private-key material).
        - verbs: [get, list, watch]
          kinds: [HTTPRoute, GRPCRoute, TCPRoute, Service, EdgionGatewayConfig]
  - name: "deploy-bot"
    token_sha256: "1111111111111111111111111111111111111111111111111111111111111111"
    rbac:
      allow:
        - verbs: [get, list, create, update, delete]
          kinds: [HTTPRoute, Service]
```

For the token generation workflow, rotation instructions, and the full security note list, see [**Federation RBAC — CLI Token RBAC**](../federation-rbac.md#cli-token-rbac).

## Federation Client (`center`)

Add the optional `center` section to join this Controller to a federation Center.
Omit it entirely when not using federation.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `address` | `String` | **Required** | gRPC address of the Center server (e.g. `https://center.example.com:12251`) |
| `name` | `String` | **Required** | Logical name of this Controller, reported to Center |
| `cluster` | `String` | **Required** | Cluster name, used for routing and aggregation |
| `enabled` | `Boolean` | `true` | Hot-reloadable kill-switch; `false` stops the federation client without a restart (reload via `POST /api/v1/reload`) |
| `env` | `Array<String>` | `[]` | Environment tags reported to Center |
| `tag` | `Array<String>` | `[]` | Extension tags reported to Center |
| `security` | Object | — | mTLS certificates for the channel; required (federation fails closed without it) |
| `rbac` | Object | built-in default | RBAC for Center operations; absent applies a built-in default policy |

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
```

For the full `center.rbac` model, default policy, and security notes, see [Federation RBAC](../federation-rbac.md).

## Related Documentation

- [Gateway Configuration](./gateway-config.md)
- [EdgionGatewayConfig CRD](./edgion-gateway-config.md)
- [Working Directory Behavior](../../dev-guide/work-directory.md)
- [Federation RBAC](../federation-rbac.md)
