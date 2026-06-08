---
name: gateway-config
description: Full schema for the edgion-gateway YAML configuration.
---

# Gateway YAML Configuration Schema

> Default file path: `config/edgion-gateway.yaml`, specified via `--config-file`.

## Files to Reference

- Sample configuration: `config/edgion-gateway.yaml`
- Configuration structure: `src/core/gateway/cli/config.rs`
- Startup logic: `src/core/gateway/cli/mod.rs`
- work_dir: `src/types/work_dir.rs`
- Common log structures: `src/types/observe.rs`, `src/types/output.rs`

## Full Schema

```yaml
# Working directory
work_dir: "."

gateway:
  server_addr: "http://127.0.0.1:12151"   # Required: Controller gRPC address
  admin_listen: "127.0.0.1:12001"         # Admin API listen address (default: 127.0.0.1:12001, loopback-only)
  # probe_listen: "0.0.0.0:12000"         # Liveness/readiness probe port (default: 0.0.0.0:12000); binds 0.0.0.0 so kubelet can reach the pod IP

# ─── gRPC channel security (optional mTLS) ───
# conf_sync_security:
#   tls:
#     cert: certs/grpc/client.crt    # Gateway client certificate
#     key: certs/grpc/client.key     # Client private key
#     ca: certs/grpc/ca.crt          # CA used to verify the Controller server
#     skip_tls: false                # true = emergency downgrade to plaintext (logs ERROR)

logging:
  # System log (not the business log)
  log_dir: logs
  log_prefix: edgion-gateway
  log_level: info                         # Module-level filtering supported: info,edgion::core::gateway::routes=debug
  json_format: false
  console: true
  buffer_size: 10000

server:
  threads: 0                               # 0 = CPU core count
  work_stealing: true                      # Tokio work-stealing
  grace_period_seconds: 30                 # Graceful shutdown wait
  graceful_shutdown_timeout_seconds: 10    # Shutdown timeout
  upstream_keepalive_pool_size: 128        # Upstream connection pool size
  downstream_keepalive_request_limit: 1000 # Maximum requests per downstream connection (0 = unlimited)
  # error_log: logs/pingora_error.log      # Pingora internal error log

# ─── Business logs ───
# Each log type has its own enable switch and path; logging.log_dir does not affect these paths

access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
      # queue_size: 80000                  # Default: cpu_cores * 10000
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

# ─── Global plugin configuration ───

rate_limit:
  default_estimator_slots_k: 64           # CMS default precision (× 64KB memory)
  max_estimator_slots_k: 1024             # CMS maximum precision cap
  gateway_instance_count: 1               # Static instance count (fallback when no Controller push)

# ─── Log redaction (off by default) ───

access_log_sensitive:
  mode: off                        # off | replace | wrap
  # replace_with: "***"            # Replacement string when mode=replace
  # wrap_prefix: "<E>"             # Prefix when mode=wrap
  # wrap_suffix: "</E>"            # Suffix when mode=wrap
```

## Field Details

### gateway

**Three-port model**: the gateway spawns three independent servers.

| Port | Default bind | Purpose |
|------|-------------|---------|
| Admin | `127.0.0.1:12001` | Admin API (`/configclient`, `/api/v1/server-info`) — loopback-only |
| Probe | `0.0.0.0:12000` | Liveness/readiness (`/health`, `/ready`) — outward-facing, kubelet probes pod IP |
| Metrics | `0.0.0.0:12090` | Prometheus metrics — independent listener, outward-facing |

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `server_addr` | `String` | **Required** | Controller gRPC address |
| `admin_listen` | `String` | `127.0.0.1:12001` | Admin API listen address. Defaults to loopback — sensitive endpoints not reachable cluster-wide. Remote access: `kubectl port-forward pod/<name> 12001:12001`. |
| `probe_listen` | `String` | `0.0.0.0:12000` | Probe listen address. Serves `/health` and `/ready` only. Binds all interfaces so kubelet can probe the pod IP directly. Not included in the gateway Service. |

### conf_sync_security.tls

Optional configuration; when enabled, the Gateway connects to the Controller's gRPC using mTLS.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `cert` | `String` | — | Client certificate path (PEM, relative to work_dir) |
| `key` | `String` | — | Client private key path (PEM) |
| `ca` | `String` | — | CA certificate path (used to verify the Controller server certificate) |
| `skip_tls` | `bool` | `false` | Emergency downgrade switch: `true` skips TLS and connects in plaintext (logs ERROR) |

> When this section is not configured, gRPC connects in plaintext, fully compatible with previous versions.
> When mTLS is enabled, the URI scheme of `gateway.server_addr` is automatically switched to `https://`.
> Generate certificates with `edgion-cli gen-certs --output file --output-dir <dir>`.

### server

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `threads` | `usize` | CPU core count | Pingora worker thread count |
| `work_stealing` | `bool` | `true` | Tokio task stealing |
| `grace_period_seconds` | `u64` | `30` | Graceful shutdown wait time in seconds |
| `graceful_shutdown_timeout_seconds` | `u64` | `10` | Shutdown timeout in seconds |
| `upstream_keepalive_pool_size` | `usize` | `128` | Upstream keepalive connection pool size |
| `downstream_keepalive_request_limit` | `u32` | `1000` | Maximum requests per downstream connection (0 = unlimited) |
| `error_log` | `String?` | — | Pingora internal error log path |

> Note: these are process-level default behaviors. If you are tuning GatewayClass-level server behavior, look at `EdgionGatewayConfig` instead.

### Business Log Schema

All business logs (access_log / ssl_log / tcp_log / tls_log / udp_log) share the same schema:

```yaml
LogConfig:
  enabled: bool                    # Whether enabled
  output:
    localFile:                     # The only output type currently
      path: String                 # Log file path (relative to work_dir)
      queue_size: usize?           # Write queue size (default: cpu_cores × 10000)
      rotation:                    # Rotation configuration (optional)
        strategy: RotationStrategy # daily | hourly | never | { Size: u64 }
        max_files: usize           # Number of files to retain (0 = unlimited)
        check_interval_secs: u64   # Rotation check interval (default 30s)
```

**RotationStrategy values**:
| Value | Description |
|-------|-------------|
| `"daily"` | Rotate at midnight daily |
| `"hourly"` | Rotate hourly |
| `"never"` | Do not rotate |
| `{ Size: 104857600 }` | Rotate by file size (bytes, default 100 MB) |

**Important**: `logging.log_dir` only affects the system log path; it does not affect business log paths. Business log paths are configured independently in each `output.localFile.path`.

### rate_limit

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `default_estimator_slots_k` | `usize` | `64` | CMS default slot count (×1K), memory ≈ slots_k × 64KB |
| `max_estimator_slots_k` | `usize` | `1024` | CMS maximum slot cap |
| `gateway_instance_count` | `u32` | `1` | Gateway instance count (denominator for cluster rate limiting; static fallback when there is no Controller) |

### access_log_sensitive

Controls the redaction behavior of fields marked as sensitive in access log, ssl.log, and plugin log. **`mode=off` by default**: all sensitive values are emitted in plaintext (preserving historical behavior).

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | `Enum` | `"off"` | `"off"` / `"replace"` / `"wrap"` |
| `replace_with` | `String` | `"***"` | When `mode=replace`, replace the entire sensitive value |
| `wrap_prefix` | `String` | `"<E>"` | Prefix appended when `mode=wrap` |
| `wrap_suffix` | `String` | `"</E>"` | Suffix appended when `mode=wrap` |

**Behavior of the three modes** (using `client_addr=1.2.3.4` as an example):

| Mode | Output |
|------|--------|
| `off` | `"1.2.3.4"` |
| `replace` | `"***"` |
| `wrap` | `"<E>1.2.3.4</E>"` |

**Scope**:
- Access log fields: `client_addr` / `remote_addr` / `path` / `x_forwarded_for` / `sni` / client certificate `subject` / `sans` / `cn`.
- Plugin log: parameters annotated via `sensitive(...)`; user parameters of DSL `log_warn` / `log_error`.
- ssl.log: client certificate fields in mTLS allowlist validation failure errors.

**Implementation details**:
- A global `AtomicU8` controls the mode, written once by `init_sensitive` at startup; immutable at runtime.
- When `off` (the default), there is only a single atomic load overhead, effectively zero cost.
- Wrap mode uses a thread-local buffer with no heap allocation.
- For custom wrap prefixes/suffixes, the total length should not exceed 36 bytes; otherwise it reduces the per-entry writable length of the plugin log.

## `integration_testing_mode`

This is a CLI flag, not a YAML config section:

```bash
./target/debug/edgion-gateway --config-file config/edgion-gateway.yaml --integration-testing-mode
```

When enabled, it additionally activates:
- Access Log Store
- Test-only metrics tags
- `/api/v1/testing/*` debug endpoints

## Most Common Gateway Configuration Changes in This Project

- Change the Controller gRPC address: change `gateway.server_addr`
- Change system log noise: change `logging.log_level`
- Toggle business logs: change `access_log` / `ssl_log` / `tcp_log` / `udp_log`
- Tune Pingora workers and keepalive: change `server`
- Tune the default RateLimit precision: change `rate_limit`
