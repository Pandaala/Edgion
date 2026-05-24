# Gateway Configuration

edgion-gateway reads its configuration from a YAML file. The default path is `config/edgion-gateway.yaml`; use `--config-file` to specify a different location.

## Full Example

```yaml
# Working directory — all relative paths are resolved against this
# Priority: CLI --work-dir > env EDGION_WORK_DIR > this value > "."
work_dir: "."

gateway:
  server_addr: "http://127.0.0.1:50051"  # Required: Controller gRPC address
  admin_listen: "0.0.0.0:5810"           # Admin API listen address (default: 0.0.0.0:5810)

# ─── gRPC channel security (optional mTLS) ────────────────────────
# conf_sync_security:
#   tls:
#     cert: certs/grpc/client.crt   # Gateway client certificate (PEM)
#     key: certs/grpc/client.key    # Client private key (PEM)
#     ca: certs/grpc/ca.crt         # CA to verify Controller server certificate
#     skip_tls: false               # true = emergency plaintext fallback, logs ERROR

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
  work_stealing: true                      # Enable Tokio work-stealing
  grace_period_seconds: 30                 # Wait for connections to drain on shutdown
  graceful_shutdown_timeout_seconds: 10    # Force-terminate after this timeout
  upstream_keepalive_pool_size: 128        # Upstream keepalive connection pool size
  downstream_keepalive_request_limit: 1000 # Max requests per downstream connection (0 = unlimited)
  # error_log: logs/pingora_error.log      # Pingora internal error log path

# ─── Business logs ────────────────────────────────────────────────
# Each log type has its own path; logging.log_dir does NOT affect these paths

access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
      # queue_size: 80000                  # Default: cpu_cores × 10000
      # rotation:
      #   strategy: daily                  # daily | hourly | never | { Size: 104857600 }
      #   max_files: 7
      #   check_interval_secs: 30

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

rate_limit:
  default_estimator_slots_k: 64   # CMS default precision (× 1K slots)
  max_estimator_slots_k: 1024     # CMS maximum precision cap
  gateway_instance_count: 1       # Cluster rate limiting: instance count (static fallback when no Controller push)

# ─── Plugin metadata ──────────────────────────────────────────────

plugin_metadata:
  # IMPORTANT: setting this field REPLACES the default list entirely — it is not appended.
  # To preserve the default protection, include all default keywords plus your custom ones.
  ctx_log_denylist: [auth, pass, claim, payload, token, secret, credential]
```

## Configuration Parameters

### [gateway]

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `server_addr` | `String` | **Required** | Controller gRPC address, e.g. `http://127.0.0.1:50051` |
| `admin_listen` | `String` | `0.0.0.0:5810` | Admin API listen address; default `0.0.0.0:5810`. Gateway Metrics API remains fixed at `:5901`. |

### [conf_sync_security.tls]

Optional. When present, Gateway connects to Controller gRPC using mTLS. When absent, gRPC runs in plaintext (compatible with older versions).

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `cert` | `String` | — | Client certificate path (PEM, relative to work_dir) |
| `key` | `String` | — | Client private key path (PEM) |
| `ca` | `String` | — | CA certificate path used to verify the Controller server certificate |
| `skip_tls` | `Boolean` | `false` | Emergency fallback: `true` skips TLS and connects in plaintext (logs ERROR) |

When mTLS is enabled, the URI scheme of `[gateway].server_addr` is automatically switched to `https://`. Certificates can be generated with `edgion-cli gen-certs --output file --output-dir <dir>`.

### [logging]

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `log_dir` | `String` | `logs` | System log directory. **Does not affect business log paths** |
| `log_prefix` | `String` | `edgion-gateway` | Log file name prefix |
| `log_level` | `String` | `info` | Log level: `trace`, `debug`, `info`, `warn`, `error`. Supports module-level filtering |
| `json_format` | `Boolean` | `false` | Emit structured JSON logs |
| `console` | `Boolean` | `true` | Also write logs to stdout |
| `buffer_size` | `Integer` | `10000` | Async log buffer size |

### [server]

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `threads` | `Integer` | CPU core count | Pingora worker thread count; `0` means auto-detect |
| `work_stealing` | `Boolean` | `true` | Enable Tokio work-stealing to improve CPU utilization |
| `grace_period_seconds` | `Integer` | `30` | Seconds to wait for connections to drain during graceful shutdown |
| `graceful_shutdown_timeout_seconds` | `Integer` | `10` | Force-terminate after this many seconds during shutdown |
| `upstream_keepalive_pool_size` | `Integer` | `128` | Upstream keepalive connection pool size |
| `downstream_keepalive_request_limit` | `Integer` | `1000` | Max requests per downstream connection; `0` means unlimited |
| `error_log` | `String` | — | Pingora internal error log file path |

> These are process-level defaults. To adjust server behavior at the GatewayClass level, use [EdgionGatewayConfig](./edgion-gateway-config.md).

### Business Logs

All business log types (`access_log`, `ssl_log`, `tcp_log`, `tls_log`, `udp_log`) share the same schema:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enabled` | `Boolean` | — | Whether to enable this log type |
| `output.localFile.path` | `String` | — | Log file path (relative to work_dir); independent of `[logging].log_dir` |
| `output.localFile.queue_size` | `Integer` | CPU cores × 10000 | Write queue size |
| `output.localFile.rotation.strategy` | `Enum` | — | Rotation strategy (see table below) |
| `output.localFile.rotation.max_files` | `Integer` | — | Number of rotated files to retain; `0` means unlimited |
| `output.localFile.rotation.check_interval_secs` | `Integer` | `30` | Rotation check interval in seconds |

**`rotation.strategy` values:**

| Value | Description |
|-------|-------------|
| `"daily"` | Rotate at midnight each day |
| `"hourly"` | Rotate every hour |
| `"never"` | No rotation |
| `{ Size = 104857600 }` | Rotate by file size (bytes), default 100 MB |

### [rate_limit]

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `default_estimator_slots_k` | `Integer` | `64` | CMS default slot count (× 1K); memory ≈ `slots_k × 64 KB` |
| `max_estimator_slots_k` | `Integer` | `1024` | Maximum CMS slot count cap |
| `gateway_instance_count` | `Integer` | `1` | Gateway instance count used as divisor for cluster rate limiting. **Must be set correctly for multi-instance deployments** |

### [plugin_metadata]

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `ctx_log_denylist` | `Array<String>` | `["auth","pass","claim","payload","token","secret","credential"]` | Ctx keys containing any of these substrings (case-insensitive) are excluded from access log |

**Important:** Setting this field **completely replaces** the default list — it is not appended. To preserve the default protection, include all default keywords plus any custom ones.

Filtering rules (either condition causes the entry to be skipped):

1. Key starts with `_` (plugin-private convention)
2. Key contains any substring in `ctx_log_denylist` (case-insensitive)

## Common Scenarios

### Connecting to a Remote Controller

```yaml
gateway:
  server_addr: "http://10.0.0.5:50051"
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
        max_files: 30
        check_interval_secs: 30
```

### Cluster Rate Limiting Configuration

When running multiple Gateway replicas, set `gateway_instance_count` to ensure accurate rate limit accounting:

```yaml
rate_limit:
  default_estimator_slots_k: 64
  max_estimator_slots_k: 1024
  gateway_instance_count: 3   # Running 3 Gateway replicas
```

### Extending the Sensitive Field Denylist

```yaml
plugin_metadata:
  # Keep all defaults and add "tenant"
  ctx_log_denylist: [auth, pass, claim, payload, token, secret, credential, tenant]
```

### Enabling mTLS to Controller

```yaml
gateway:
  server_addr: "http://10.0.0.5:50051"  # scheme is automatically switched to https://

conf_sync_security:
  tls:
    cert: certs/grpc/client.crt
    key: certs/grpc/client.key
    ca: certs/grpc/ca.crt
```

## Related Documentation

- [Controller Configuration](./controller-config.md)
- [EdgionGatewayConfig CRD](./edgion-gateway-config.md)
- [Access Log Format](../observability/access-log.md)
- [Working Directory Behavior](../../dev-guide/work-directory.md)
