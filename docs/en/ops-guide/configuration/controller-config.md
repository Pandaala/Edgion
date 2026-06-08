# Controller Configuration

edgion-controller reads its configuration from a YAML file. The default path is `config/edgion-controller.yaml`; use `--config-file` to specify a different location.

## Full Example

```yaml
# Working directory — all relative paths are resolved against this
# Priority: CLI --work-dir > env EDGION_WORK_DIR > this value > "."
work_dir: "."

server:
  grpc_listen: "0.0.0.0:12151"   # ConfigSyncServer gRPC listen address
  admin_listen: "0.0.0.0:12101"  # Admin HTTP API address (/api/v1, /configclient)
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

# ─── gRPC channel security (optional mTLS) ────────────────────────
# conf_sync_security:
#   tls:
#     cert: certs/grpc/server.crt   # Controller server certificate (PEM)
#     key: certs/grpc/server.key    # Server private key (PEM)
#     ca: certs/grpc/ca.crt         # CA to verify Gateway client certificates
#     skip_tls: false               # true = emergency plaintext fallback, logs ERROR

# ─── conf_center: FileSystem mode (local development) ─────────────
conf_center:
  type: file_system
  conf_dir: config/resources      # YAML resource directory (prefer absolute paths)

# ─── conf_center: Kubernetes mode (production) ────────────────────
# conf_center:
#   type: kubernetes
#   gateway_class: edgion                          # Required: GatewayClass name to manage
#   controller_name: edgion.io/gateway-controller  # controllerName reported in Gateway API status
#   watch_namespaces: []                           # Empty = watch all namespaces
#   label_selector: ""                             # Label filter for watched resources
#   gateway_address: ""                            # Address reported in Gateway status
#   ha_mode: leader-only                           # "leader-only" | "all-serve"
#   no_watch_kinds: [TCPRoute, UDPRoute]           # Skip CRD pre-check and watch for these kinds
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
| `admin_listen` | `String` | `0.0.0.0:12101` | Admin HTTP API listen address — serves `/api/v1/*` and `/configclient` |
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

### [conf_sync_security.tls]

Optional. When present, the Controller gRPC port uses mTLS. When absent, gRPC runs in plaintext (compatible with older versions).

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `cert` | `String` | — | Server certificate path (PEM, relative to work_dir) |
| `key` | `String` | — | Server private key path (PEM) |
| `ca` | `String` | — | CA certificate path used to verify Gateway client certificates |
| `skip_tls` | `Boolean` | `false` | Emergency fallback: `true` skips TLS and starts in plaintext mode (logs ERROR) |

Certificates can be generated with `edgion-cli gen-certs --output file --output-dir <dir>`.

`conf_center.type` selects the backend. Accepted values are `file_system`,
`kubernetes`, and `etcd` (snake_case — `file_system`, not `filesystem`).

### [conf_center] — FileSystem Mode

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `type` | `String` | — | Must be `file_system` |
| `conf_dir` | `Path` | — | YAML resource directory. **Use absolute paths** — the current implementation does not resolve this relative to `work_dir` |

Note: The `--conf-dir` CLI flag overrides `conf_dir` and switches `conf_center` to FileSystem mode.

### [conf_center] — Kubernetes Mode

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `type` | `String` | — | Must be `"kubernetes"` |
| `gateway_class` | `String` | **Required** | GatewayClass name this controller manages |
| `controller_name` | `String` | — | `controllerName` reported in Gateway API status |
| `watch_namespaces` | `Array<String>` | `[]` (all) | Namespaces to watch; empty list means all namespaces |
| `label_selector` | `String` | `""` | Label selector filter for watched resources |
| `gateway_address` | `String` | — | Address reported in Gateway status. Optional |
| `ha_mode` | `String` | — | HA mode: `"leader-only"` or `"all-serve"` |
| `no_watch_kinds` | `Array<String>` | `[]` | Resource kinds to skip CRD pre-check and watch for (PascalCase), e.g. `["TCPRoute", "UDPRoute"]` |

### [conf_center.leader_election]

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `lease_name` | `String` | `edgion-controller-leader` | Kubernetes Lease resource name |
| `lease_namespace` | `String` | `edgion-system` | Namespace where the Lease lives |
| `lease_duration_secs` | `Integer` | `15` | Lease hold duration in seconds (server-side validity on the apiserver) |
| `renew_deadline_secs` | `Integer` | `10` | Renewal deadline window: the leader keeps leadership across transient errors as long as the last successful renew was within this window. Constraint: `retry_period_secs < renew_deadline_secs < lease_duration_secs` |
| `retry_period_secs` | `Integer` | `2` | Sleep interval between each acquire/renew attempt (client-go `RetryPeriod`) |

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
  gateway_class: edgion
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
  gateway_class: edgion
  ha_mode: leader-only
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

## Authentication

Authentication is **mandatory** for the Controller Admin API. All requests to `/api/v1/*` routes require a valid Bearer token or session cookie. Unauthenticated requests are rejected with HTTP 401.

> **Credentials must be supplied explicitly.** The Controller no longer auto-generates admin
> credentials. Credentials come from (highest precedence first):
>
> 1. Env vars `EDGION_ADMIN_USERNAME` / `EDGION_ADMIN_PASSWORD` / `EDGION_ADMIN_JWT_SECRET`
>    injected via `valueFrom.secretKeyRef` from the `edgion-admin-credentials` Secret
>    (recommended for Kubernetes). `deploy.sh` creates this Secret idempotently.
> 2. `[local_auth]` in a Secret-mounted config file.
> 3. `[auth]` OIDC.
>
> When nothing is configured, the server starts but all Admin API business routes return
> **503 fail-close**. `/health`, `/ready`, `/metrics`, and `/api/v1/auth/status` remain
> accessible. Retrieve the password (if using `deploy.sh`):
>
> ```bash
> kubectl -n edgion-system get secret edgion-admin-credentials \
>   -o jsonpath='{.data.password}' | base64 -d
> ```
>
> You can also authenticate interactively with `edgion-cli login`.

The Controller and Center read the same `[local_auth]` / `[auth]` schema and now bootstrap identically: when neither is set, both fail closed with 503 (see the Center note below).

### [local_auth]

Built-in username/password authentication. Issues HS256 JWTs to authenticated clients. Must be configured explicitly on both Controller and Center (no auto-generation).

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `username` | `String` | `"admin"` | Admin username |
| `password` | `String` | _(required)_ | Admin password. Must be set explicitly; if absent the Admin API returns 503 |
| `jwt_secret` | `String` | _(required)_ | JWT signing secret. Must be set explicitly; if absent the Admin API returns 503 |
| `jwt_expiry_hours` | `Integer` | `24` | Issued token lifetime in hours |
| `cookie_secure` | `Boolean` | `true` | Emit the `Secure` attribute on the `edgion_token` auth cookie. Keep `true` for any TLS-fronted deployment so the bearer-equivalent cookie never traverses plaintext `http://`. Set `false` only for an explicit non-TLS dev/internal setup served over plain HTTP. |

```yaml
local_auth:
  username: admin
  password: "change-me"
  jwt_secret: "a-long-random-signing-secret"
  jwt_expiry_hours: 24
  cookie_secure: true
```

> **Note**: Both Controller and Center behave identically — neither auto-generates credentials. If neither `[local_auth]` nor `[auth]` is configured, business routes return 503 (fail-close) while `/health`, `/ready`, `/metrics`, and `/api/v1/auth/status` remain accessible.

### [auth] — OIDC

Optional OIDC integration for the Admin API. Validates JWT Bearer tokens against an OIDC provider's discovery document and JWKS. Can coexist with `[local_auth]`; requests are attempted in the order OIDC → local.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enabled` | `Boolean` | `true` (when the section is present) | Enable OIDC validation |
| `discovery` | `String` | **Required** | OIDC discovery URL (`/.well-known/openid-configuration`) |
| `audiences` | `Array<String>` | `[]` | Expected JWT `aud` values; empty skips the audience check |
| `issuers` | `Array<String>` | `[]` | Expected JWT `iss` values; empty validates against the discovery document's issuer |
| `allowed_algorithms` | `Array<String>` | `[RS256, RS384, RS512, ES256, ES384]` | Allowed signing algorithms |
| `clock_skew_seconds` | `Integer` | `120` | JWT timestamp tolerance in seconds |
| `jwks_cache_ttl` | `Integer` | `300` | JWKS cache TTL in seconds |
| `jwks_min_refresh_interval` | `Integer` | `10` | Minimum interval between JWKS refresh attempts (debounce), seconds |
| `skip_paths` | `Array<String>` | `["/health", "/ready"]` | Path prefixes that skip authentication |
| `ssl_verify` | `Boolean` | `true` | Verify TLS certificates when fetching discovery/JWKS; production must keep `true` |
| `discovery_max_response_bytes` | `Integer` | `65536` | Upper bound for the discovery response body |
| `jwks_max_response_bytes` | `Integer` | `1048576` | Upper bound for the JWKS response body |

```yaml
auth:
  discovery: "https://keycloak.example.com/realms/edgion/.well-known/openid-configuration"
  audiences: [edgion-admin]
```

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
| `ping_interval_secs` | `Integer` | `30` | Federation keep-alive ping interval |
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
