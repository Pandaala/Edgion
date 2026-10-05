# Gateway Configuration

edgion-gateway reads its configuration from a YAML file. The default path is `config/edgion-gateway.yaml`; use `--config-file` to specify a different location.

## Full Example

```yaml
# Working directory — all relative paths are resolved against this
# Priority: CLI --work-dir > env EDGION_WORK_DIR > this value > "."
work_dir: "."

gateway:
  server_addrs: ["http://127.0.0.1:12151"]  # Required: Controller gRPC address
  admin_listen: "127.0.0.1:12001"        # Admin API listen address (default: 127.0.0.1:12001, loopback-only)
  # probe_listen: "0.0.0.0:12000"        # Liveness/readiness probe port (default: 0.0.0.0:12000); binds 0.0.0.0 so kubelet can reach the pod IP
  # geoip:                               # MUST be nested under `gateway:` — a top-level `geoip:` key is
  #                                       # ignored with a startup ERROR log. See docs/en/ops-guide/gateway/geoip.md
  #   db_path: /var/lib/edgion/geoip/db.mmdb
  #   update_url: https://mirror.internal/geo.mmdb

# Unknown fields in this process YAML are ignored for rollout compatibility, but the Gateway
# emits a startup ERROR log with the full field path and action="ignored". Existing strict subtrees
# (including geoip, path_normalization, request_mirror, and access_log_options) still reject
# unknown fields and abort startup.

# ─── gRPC channel security (REQUIRED) ─────────────────────────────
# Omitting this section makes the Gateway REFUSE TO START. Pick exactly one of the
# three options below, and keep it consistent with the Controller's configuration.
#
# Option 1 — Kubernetes: the Controller's internal CA provisions the client cert.
# NOTE: on the GATEWAY, `auto_ca` alone is NOT enough — auto_ca implies mTLS, so the cert
# paths must also be set (the Gateway exits non-zero with "auto_ca=true but no cert paths
# are configured" otherwise). They point at the Secret the Controller provisions
# (`edgion-conf-sync-certs`), mounted at /etc/edgion/conf-sync-certs/ by the shipped
# manifests. Only the CONTROLLER may set a bare `auto_ca: true` (it mints its own identity
# in memory).
conf_sync_security:
  auto_ca: true
  tls:
    cert: /etc/edgion/conf-sync-certs/client.crt
    key: /etc/edgion/conf-sync-certs/client.key
    ca: /etc/edgion/conf-sync-certs/ca.crt
#
# Option 2 — file-mode mTLS:
# conf_sync_security:
#   tls:
#     cert: certs/grpc/client.crt   # Gateway client certificate (PEM)
#     key: certs/grpc/client.key    # Client private key (PEM)
#     ca: certs/grpc/ca.crt         # CA to verify Controller server certificate
#
# Option 3 — explicit plaintext opt-in (local dev / non-production only, logs ERROR):
# conf_sync_security:
#   tls:
#     skip_tls: true

logging:
  # System logs only — does NOT affect business log paths
  log_dir: logs
  log_prefix: edgion-gateway
  log_level: info               # Module-level supported: info,edgion::core::gateway::routes=debug
  json_format: false
  console: true
  buffer_size: 10000

server:
  threads: 0                               # 0 = auto-detect CPU core count
  # downstream_tls_offload_threadpools: 2       # Optional, 1..=4
  # downstream_tls_offload_thread_per_pool: 2  # Optional, 1..=4; both fields required
  work_stealing: true                      # Enable Tokio work-stealing
  grace_period_seconds: 30                 # Wait for connections to drain on shutdown
  graceful_shutdown_timeout_seconds: 10    # Force-terminate after this timeout
  upstream_keepalive_pool_size: 128        # Upstream keepalive connection pool size
  # error_log: logs/pingora_error.log      # Pingora internal error log path

# ─── Business logs ────────────────────────────────────────────────
# Each log type has its own path; logging.log_dir does NOT affect these paths

access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
      # queueSize: 80000                   # Default: cpu_cores × 10000
      # rotation:
      #   strategy: daily                  # daily | hourly | never | { Size: 104857600 }
      #   maxFiles: 7
      #   checkIntervalSecs: 30

ssl_log:
  enabled: true
  output:
    localFile:
      path: logs/ssl.log

tcp_log:
  enabled: false
  output:
    localFile:
      path: logs/tcp_access.log

tls_log:
  enabled: true
  output:
    localFile:
      path: logs/tls_access.log

udp_log:
  enabled: false
  output:
    localFile:
      path: logs/udp_access.log

# ─── Rate limiting plugin ──────────────────────────────────────────

rate_limit_local:
  default_estimator_slots_k: 64   # CMS default precision (× 1K slots)
  max_estimator_slots_k: 1024     # CMS maximum precision cap
  gateway_instance_count: 1       # Cluster rate limiting: instance count (static fallback when no Controller push)

# ─── OIDC introspection TinyUFO limits ────────────────────────────

tiny_ufo_ttl_cache:
  default_max_entries: 1024
  default_max_logical_bytes: 4MiB

# LRU defaults, including OIDC access-token caching.
lru_cache:
  default_capacity: 1024
  default_shards: 0

# ─── Log redaction (replace by default) ────────────────────────────

access_log_sensitive:
  mode: replace                   # off | replace | wrap
  # replace_with: "***"           # Replacement string when mode=replace
  # wrap_prefix: "<E>"            # Prefix when mode=wrap
  # wrap_suffix: "</E>"           # Suffix when mode=wrap
```

## Configuration Parameters

### gateway

The gateway uses a **three-port model**:

| Port | Default bind | Purpose |
|------|-------------|---------|
| Admin | `127.0.0.1:12001` | Admin API (`/configclient`, `/api/v1/server-info`, etc.) — loopback-only, never externally reachable |
| Probe | `0.0.0.0:12000` | Liveness and readiness only (`/health`, `/ready`) — binds all interfaces so kubelet can reach the pod IP |
| Metrics | `0.0.0.0:12090` | Prometheus metrics — independent listener, outward-facing |

The probe port is intentionally **not** added to the gateway Service; kubelet probes the pod IP directly on port 12000.

> **Breaking change for operators**: remote access to the gateway admin API (`/configclient`, `/api/v1/server-info`) via pod IP or a Kubernetes Service is removed. Use `kubectl port-forward pod/<gw-pod> 12001:12001` (targets the pod loopback) or `kubectl exec` instead. Do **not** use the gateway Service for this. `edgion-cli --target client` defaults to `localhost:12001` and is unaffected.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `server_addrs` | `Array<String>` | **Required** | Controller conf_sync gRPC addresses, e.g. `["http://127.0.0.1:12151"]`. A single address reconnects in place; multiple addresses enable candidate probing and failover according to the Controller's serving mode. See [Controller failover](#controller-failover). `--server-addr` is repeatable on the CLI |
| `admin_listen` | `String` | `127.0.0.1:12001` | Admin API listen address. Defaults to loopback (`127.0.0.1:12001`) — sensitive endpoints (`/configclient`, `/api/v1/server-info`) are not exposed cluster-wide. Access remotely via `kubectl port-forward pod/<name> 12001:12001`. |
| `admin_token` | `String` | — | Shared bearer token for the admin API (config-file fallback). Prefer the `EDGION_GATEWAY_ADMIN_TOKEN` env var in Kubernetes. Never serialized out, redacted in logs. See [Gateway admin API authentication](#gateway-admin-api-authentication). |
| `probe_listen` | `String` | `0.0.0.0:12000` | Liveness/readiness probe listen address. Binds all interfaces so kubelet can reach the pod IP. Serves only `/health` and `/ready`. Not included in the gateway Service. |
| `geoip` | `GeoIpConfig` | *(empty — no `db_path`)* | GeoIP database + updater settings, used by the `GeoIpLocation` plugin (HTTP `EdgionPlugin` and L4 `EdgionStreamPlugin`, one config shape). **Must be nested under `gateway:`** — see [GeoIP Restriction](../gateway/geoip.md) for the full field reference and required pitfalls. |

### Controller failover

With multiple `server_addrs`, the Gateway shuffles the list once and keeps one
active configuration-sync session. When that session fails, it probes candidates
on demand and switches only after a candidate answers `GetServerInfo` successfully.
It reuses the verified channel for synchronization. A reachable Controller that
cannot currently serve configuration is not selected: leader-only deployments
require the serving leader, while all-serve deployments may use any ready replica.
The Controller enforces this eligibility; a successful probe does not reserve its
leadership or guarantee that the following stream will succeed.

Every configured address is checked for valid gRPC URI syntax at startup. An
invalid entry fails startup even if another entry is usable; fix the address list
before restarting. This check does not require the remote endpoints to be online.

Unavailable candidates do not replace the installed channel. Recovery walks the
address list, including the original address, and backs off after complete failed
remote laps. Local certificate/channel preparation errors preserve the candidate
for retry. The Gateway continues using its existing configuration during recovery;
a new Gateway still needs its initial configuration before it can serve traffic.
With one address, it reconnects to that address without endpoint rotation.

Certificate reload rebuilds the currently installed address with validated local
material, even when the Controller is unreachable. A concurrent probe made against
an older connection generation cannot overwrite that reload. No additional health
RPC, persistent standby fleet, or operator setting is required.

### Gateway admin API authentication

The admin API (`gateway.admin_listen`, default `127.0.0.1:12001`) is protected by a
**static bearer token**. Every admin route — including the integration-testing /
destructive routes — requires `Authorization: Bearer <token>`. The probe port
(`:12000`, `/health` `/ready`) is intentionally left auth-free.

**Configuring the token (highest precedence first):**

1. **`EDGION_GATEWAY_ADMIN_TOKEN` env var** — recommended for Kubernetes. Injected
   from the `edgion-gateway-admin-credentials` Secret (key `token`) via
   `secretKeyRef`. `deploy.sh` creates this Secret idempotently with a random value
   (`openssl rand -hex 32`); an existing Secret is left untouched.
2. **`gateway.admin_token` config-file field** — fallback for a Secret-mounted
   config file only. It is never serialized out and is redacted in `Debug` / logs.

**Reading the token** (for `edgion-cli` or `curl`):

```bash
kubectl -n <ns> get secret edgion-gateway-admin-credentials \
  -o jsonpath='{.data.token}' | base64 -d
```

```bash
curl -H "Authorization: Bearer $(kubectl -n <ns> get secret \
  edgion-gateway-admin-credentials -o jsonpath='{.data.token}' | base64 -d)" \
  http://127.0.0.1:12001/configclient/gateway/list
```

**Fail-close behavior:**

| Condition | Response |
|-----------|----------|
| No token configured server-side | `503` on all admin routes (with a startup `WARN`) |
| Missing or wrong token | `401` |
| Correct token | request proceeds |

> **Security caveat:** a non-loopback admin bind (`admin_listen: 0.0.0.0:...`)
> sends a **replayable, non-expiring static token in cleartext** unless fronted by
> mesh mTLS. A `NetworkPolicy` is L3/L4 access control, **not** confidentiality.
> Require mesh mTLS for any non-loopback exposure. The token has no `exp` — on any
> suspected exposure, rotate the Secret and restart the pod immediately:
>
> ```bash
> kubectl -n <ns> delete secret edgion-gateway-admin-credentials
> # re-run deploy.sh (or recreate the Secret), then:
> kubectl -n <ns> rollout restart deployment/edgion-gateway
> ```

### conf_sync_security

**Required.** The conf_sync gRPC channel delivers resolved Secrets (TLS private keys, backend
credentials) to the Gateway, so its transport must be chosen explicitly.

**Omitting the `conf_sync_security` section entirely makes the Gateway refuse to start**
(non-zero exit, with an actionable error). Configure exactly one of:

| Configuration | Result |
|---------------|--------|
| `auto_ca: true` **plus** `tls` cert paths | mTLS — the Controller's internal CA provisions the client cert (recommended on Kubernetes) |
| `auto_ca: true` **alone**, no cert paths | **Refuses to start** — `auto_ca` implies mTLS, so the Gateway needs the paths to the provisioned cert files |
| `certs` / legacy `tls` block with `cert` + `key` + `ca` | File-mode mTLS |
| `tls.skip_tls: true` | Plaintext, explicitly opted in (logs ERROR; local dev / non-production only) |
| *(section omitted)* | **Refuses to start** |

> **`auto_ca` is asymmetric — do not copy the Controller's block.** The **Controller** needs
> only `auto_ca: true`: it mints its own server identity in memory. The **Gateway** needs
> `auto_ca: true` **and** the `tls` cert paths, because its client cert arrives as a
> Controller-provisioned Secret that must be read from disk. A bare `auto_ca: true` on the
> Gateway exits non-zero with `conf_sync_security.auto_ca=true but no cert paths are
> configured`.

The choice must match the Controller's — a mismatch causes a handshake failure.

> **Migration**: omitting the section used to mean "plaintext gRPC". Deployments that relied on
> that will now fail to start. Pick one:
>
> ```yaml
> # (a) Kubernetes — auto_ca + the provisioned cert paths (matches the shipped ConfigMap)
> conf_sync_security:
>   auto_ca: true
>   tls:
>     cert: /etc/edgion/conf-sync-certs/client.crt
>     key: /etc/edgion/conf-sync-certs/client.key
>     ca: /etc/edgion/conf-sync-certs/ca.crt
>
> # (b) file-mode mTLS
> conf_sync_security:
>   tls: { cert: certs/grpc/client.crt, key: certs/grpc/client.key, ca: certs/grpc/ca.crt }
>
> # (c) keep plaintext, explicitly (INSECURE)
> conf_sync_security:
>   tls: { skip_tls: true }
> ```

#### conf_sync_security.tls

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `cert` | `String` | — | Client certificate path (PEM, relative to work_dir) |
| `key` | `String` | — | Client private key path (PEM) |
| `ca` | `String` | — | CA certificate path used to verify the Controller server certificate |
| `skip_tls` | `Boolean` | `false` | Explicit plaintext opt-in: `true` skips TLS and connects in plaintext (logs ERROR and raises `edgion_system_error_state{reason="skip_tls"}`) |

When mTLS is enabled, the URI scheme of `gateway.server_addrs` is automatically switched to `https://`. Certificates can be generated with `edgion-cli gen-certs --output file --output-dir <dir>`.

### logging

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `log_dir` | `String` | `logs` | System log directory. **Does not affect business log paths** |
| `log_prefix` | `String` | `edgion-gateway` | Log file name prefix |
| `log_level` | `String` | `info` | Log level: `trace`, `debug`, `info`, `warn`, `error`. Supports module-level filtering |
| `json_format` | `Boolean` | `false` | Emit structured JSON logs |
| `console` | `Boolean` | `true` | Also write logs to stdout |
| `buffer_size` | `Integer` | `10000` | Async log buffer size |

The Gateway always caps `pingora_core::protocols::http::v1::client` and
`pingora_proxy::proxy_h1` at `error`, even if `RUST_LOG` or `log_level` requests a more verbose
level. These upstream Pingora modules contain complete HTTP/1 request-header diagnostics and
cannot be enabled below `error`; other targets retain normal module-level filtering.

### server

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `threads` | `Integer` | CPU core count | Pingora worker thread count; `0` means auto-detect |
| `downstream_h2_stream_window_size` | `Integer` | `65535` | HTTP/2 receive window per stream in bytes; `65535`–`16777216` |
| `downstream_h2_connection_window_size` | `Integer` | `65535` | HTTP/2 receive window per connection in bytes; `65535`–`16777216` |
| `downstream_h2_max_concurrent_streams` | `Integer` | `100` | Maximum simultaneous client streams per HTTP/2 connection; `1`–`1024` |
| `downstream_tls_offload_threadpools` | `Integer` | Disabled | Dedicated handshake pools per terminating TLS listener; `1`–`4`, requires the paired field |
| `downstream_tls_offload_thread_per_pool` | `Integer` | Disabled | Threads per handshake pool; `1`–`4`, requires the paired field |
| `work_stealing` | `Boolean` | `true` | Enable Tokio work-stealing to improve CPU utilization |
| `grace_period_seconds` | `Integer` | `30` | Seconds to wait for connections to drain during graceful shutdown |
| `graceful_shutdown_timeout_seconds` | `Integer` | `10` | Force-terminate after this many seconds during shutdown |
| `upstream_keepalive_pool_size` | `Integer` | `128` | Upstream keepalive connection pool size |
| `error_log` | `String` | — | Pingora internal error log file path |

> These settings configure one Gateway process at startup and require a restart to change. For
> GatewayClass-scoped HTTP compression and downstream connection request limits, use
> [EdgionGatewayConfig](./edgion-gateway-config.md).

Downstream HTTP/2 tuning applies uniformly to every HTTP listener supporting h2c and
every HTTPS listener negotiating H2. Omission preserves Pingora defaults, including
its 64 KiB decoded request-header limit. For example, to allow 256 KiB per stream
and 1 MiB per connection with up to 100 concurrent streams:

```yaml
server:
  downstream_h2_stream_window_size: 262144
  downstream_h2_connection_window_size: 1048576
  downstream_h2_max_concurrent_streams: 100
```

The effective stream window multiplied by the effective concurrent-stream limit
must not exceed `67108864` bytes (64 MiB). This is a conservative configuration
credit budget per connection, not a hard limit on process memory or total body size.
Invalid individual values or combinations fail before startup services begin. Each
field is optional; omitted fields retain their own defaults. CLI flags
`--downstream-h2-stream-window-size`, `--downstream-h2-connection-window-size`, and
`--downstream-h2-max-concurrent-streams` override the corresponding YAML fields.
Use integer byte counts, not size strings. Changes require a Gateway restart and
apply to connections established by the restarted process; existing connections
are not updated. These settings do not enable HTTP/2 on a listener where it is
disabled, do not change upstream connection windows, and leave HTTP/1 unchanged.
Larger receive windows may help uploads or client-to-server gRPC messages over
high-latency links. Measure resource use and throughput for your traffic before
increasing them; they do not directly enlarge the client's download receive window.

TLS handshake offload applies to HTTPS and TLS termination listeners. Each distinct
terminating listener owns its own pool: `listeners × pools × threads per pool` must
not exceed 64. Invalid or partial settings fail startup. The threads start on the
first handshake, so startup validation does not guarantee that thread creation will
succeed later. Offload does not cap concurrent handshakes or reserve CPU for
established traffic. Use connection admission separately when needed. Compare
off/on under the same CPU limit for your traffic before enabling it; additional
threads can hurt small or mostly reused-connection deployments. Disable both
fields and restart the Gateway to roll back. The equivalent CLI flags are
`--downstream-tls-offload-threadpools` and
`--downstream-tls-offload-thread-per-pool`; a provided flag overrides its YAML
field.

### Business Logs

All business log types (`access_log`, `ssl_log`, `tcp_log`, `tls_log`, `udp_log`) share the same schema:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enabled` | `Boolean` | — | Whether to enable this log type |
| `output.localFile.path` | `String` | — | Log file path (relative to work_dir); independent of `logging.log_dir` |
| `output.localFile.queueSize` | `Integer` | CPU cores × 10000 | Write queue size |
| `output.localFile.rotation.strategy` | `Enum` | — | Rotation strategy (see table below) |
| `output.localFile.rotation.maxFiles` | `Integer` | — | Number of rotated files to retain; `0` means unlimited |
| `output.localFile.rotation.checkIntervalSecs` | `Integer` | `30` | Check interval in seconds for the time-based strategies (`daily` / `hourly`). The `Size` strategy is checked after every write batch and is not affected by this field. |

**`rotation.strategy` values:**

| Value | Description |
|-------|-------------|
| `"daily"` | Rotate at midnight each day |
| `"hourly"` | Rotate every hour |
| `"never"` | No rotation |
| `{ Size: 104857600 }` | Rotate by file size (bytes), default 100 MB |

### request_mirror

Configure this top-level block in the Gateway process YAML:

```yaml
request_mirror:
  max_concurrent: 1024
  max_buffered_bytes: 64MiB
```

`max_concurrent` limits total active mirror tasks (`1`–`65536`).
`max_buffered_bytes` limits retained mirror request-body data (`1` byte–`1GiB`).
These are the defaults; both values are configurable and require a Gateway restart.
All routes and plugin instances share one budget, including old and new plugin
generations during hot reload. Exhaustion skips or cancels the mirror copy while
the main request continues. Invalid values and unknown keys fail startup; zero
does not disable either limit. These fields have no CLI or environment override.

The byte budget is not a total process RSS limit: request headers, TLS and
connection overhead are separate. Per-mirror limits remain configurable through
[HTTPRoute mirror annotations](../../user-guide/http-route/filters/gateway-api/request-mirror.md)
or EdgionPlugins inline configuration; standard Gateway API fields are unchanged.

### rate_limit

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `default_estimator_slots_k` | `Integer` | `64` | CMS default slot count (× 1K); memory ≈ `slots_k × 64 KB` |
| `max_estimator_slots_k` | `Integer` | `1024` | Maximum CMS slot count cap |
| `gateway_instance_count` | `Integer` | `1` | Gateway instance count used as divisor for cluster rate limiting. **Must be set correctly for multi-instance deployments** |

### access_log_sensitive

TLS proxy logs (global and per listener) mask backend `upstream_info.addr` and
the destination address inside connection errors and retries. The separate
backend port and surrounding error diagnostics remain readable. TLS proxy and
SSL handshake `sni` remain plain in every mode; the HTTP access-log SNI masking
described below is unchanged. Other TLS/SSL error text is not masked as a whole.
`wrap` marks address fragments while retaining their original values.

Controls redaction of fields marked sensitive in the access log, ssl.log, and plugin log. **`mode=replace` by default** — sensitive values render `***` out of the box. This is a wider blast radius than the built-in field list alone: the nine built-in fields (`direct_peer_ip` / `remote_ip` / `path` / `x_forwarded_for` / `sni` / `request_uri` / client-cert `subject`/`sans`/`cn`), 46 `sensitive()`-marked plugin-log parameters across 13 plugins, DSL `log_warn`/`log_error` user args, ssl.log mTLS-failure cert fields, and — new — any `ext` field declared via [`AccessLogExtern`](../observability/access-log.md) that is not whitelisted in `EdgionGatewayConfig.spec.accessLogExtern.unmaskedKeys`.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `mode` | `Enum` | `replace` | Redaction mode: `off`, `replace`, or `wrap` |
| `replace_with` | `String` | `"***"` | Replacement string used when `mode=replace` |
| `wrap_prefix` | `String` | `"<E>"` | Prefix added when `mode=wrap` |
| `wrap_suffix` | `String` | `"</E>"` | Suffix added when `mode=wrap` |

### Upstream RemoteIp header

`EdgionGatewayConfig.spec.forwardedHeaders.remoteIpHeader` controls the header
that the Gateway writes on upstream HTTP requests after the final request-stage
`RemoteIp` has been resolved. It defaults to `X-Real-IP`, matching the existing
behavior and the common Nginx convention. Operators may choose a custom header,
for example `X-Edgion-Remote-IP`. The configured name is validated and cannot be
a hop-by-hop header, `Host`, `Content-Length`, `X-Forwarded-For`, or another
reserved transport header. The Gateway overwrites an incoming value with its
canonical value after all request plugins have run.

This output setting is independent from `spec.realIp.realIpHeader`, which names
the downstream header read by the RealIp extractor. The former propagates the
resolved identity upstream; the latter establishes it from a trusted proxy.

Behavior (using `direct_peer_ip=192.168.1.10`):

| Mode | Output |
|------|--------|
| `off` | `192.168.1.10` |
| `replace` | `***` |
| `wrap` | `<E>192.168.1.10</E>` |

**There is no configuration that gives readable `path` + masked `AccessLogExtern` cookie.** `off` makes the whitelist inert (every `ext` field clear); `wrap` writes the raw value into the local log file/stdout (tagged for a downstream ES/ClickHouse pipeline to drop, but present locally). Pick one of the two trade-offs deliberately — this is a design choice, not a gap.

**The repo's own `config/edgion-gateway.yaml` — the file this page documents as the default path — ships `mode: off`, not the `replace` default described above.** That file is the local-dev and integration-harness config; the integration suites assert on `path` and on plugin-log values written through `sensitive(...)`, which need real, unmasked values. A config file with no `access_log_sensitive` section at all gets the `replace` default; running the binary against the repo's shipped file does not, because that file's `access_log_sensitive.mode` is set explicitly. The release image bakes no config (`cicd/docker/Dockerfile.runtime`), so a Kubernetes deployment is unaffected by this file.

## Common Scenarios

### Connecting to a Remote Controller

```yaml
gateway:
  server_addrs: ["http://10.0.0.5:12151"]
```

### Enabling Daily Log Rotation for Access Logs

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
      rotation:
        strategy: daily
        maxFiles: 30
        checkIntervalSecs: 30
```

### Cluster Rate Limiting Configuration

When running multiple Gateway replicas, set `gateway_instance_count` to ensure accurate rate limit accounting:

```yaml
rate_limit_local:
  default_estimator_slots_k: 64
  max_estimator_slots_k: 1024
  gateway_instance_count: 3   # Running 3 Gateway replicas
```

### OIDC Introspection TinyUFO Limits

```yaml
tiny_ufo_ttl_cache:
  default_max_entries: 1024
  default_max_logical_bytes: 4MiB
```

These process defaults apply when OIDC introspection omits its
`tinyUfoTtlCache` override. An override must set `maxEntries` and
`maxLogicalBytes` together. The total logical budget controls TinyUFO record
weights for one cache instance and is not an exact allocator or RSS limit;
enforce the Gateway process hard limit with Kubernetes/cgroup memory limits.
OIDC access-token caching and the data-plane `ExternalName` DNS cache use the
independent `lru_cache` defaults; those defaults are what bounds the DNS cache's
retention across Service, hostname, and resolver churn. The DNS cache takes the
capacity but applies its own shard floor, because it is read on every
`ExternalName` request — setting `default_shards` below that floor does not lower
its shard count. LDAP authentication is never cached.
Absolute safety ceilings are 65,536 cache entries, 4,194,304 TinyUFO weight
units (256 MiB of 64-byte logical units). These ceilings are not defaults.

### Redacting Sensitive Fields in Access Logs

`mode: replace` is already the default, so the sensitive built-in fields (client IPs, SNI, client-certificate fields, `AccessLogExtern` `ext` fields not on the whitelist, etc.) are masked out of the box. Only add the block explicitly if you want to override `replace_with`, or opt back into plaintext:

```yaml
access_log_sensitive:
  mode: replace
  replace_with: "***"
```

### Enabling mTLS to Controller

```yaml
gateway:
  server_addrs: ["http://10.0.0.5:12151"]  # scheme is automatically switched to https://

conf_sync_security:
  tls:
    cert: certs/grpc/client.crt
    key: certs/grpc/client.key
    ca: certs/grpc/ca.crt
```

## Related Documentation

- [Controller Configuration](./controller-config.md)
- [EdgionGatewayConfig CRD](./edgion-gateway-config.md)
- [GeoIP Restriction](../gateway/geoip.md)
- [Access Log Format](../observability/access-log.md)
- [Working Directory Behavior](../../dev-guide/work-directory.md)
