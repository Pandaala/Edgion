---
name: cli-and-startup
description: Command-line conventions, working-directory layout, config-file path conventions, log initialization, and signal handling shared by the in-repo binaries.
---

# Unified command-line and configuration conventions

## Entry-point pattern

All in-repo binaries follow the **thin entry point** pattern — `src/bin/*.rs` does the minimum amount of work:

| bin | Entry logic |
|-----|---------|
| edgion-controller | Install rustls crypto provider → `parse_args()` → `run()` |
| edgion-gateway | Install rustls crypto provider → `parse_args()` → `run()` (plain `fn main()`, manages its own Tokio runtime) |
| edgion-cli | `Cli::parse()` → `cli.run().await` (`#[tokio::main]`) |

## Command-line arguments

### Shared flags

| Flag | Controller | Gateway | ctl | Default |
|------|:---------:|:-------:|:---:|--------|
| `--work-dir` / `-w` | ✓ | ✓ | ✗ | `"."` |
| `--config-file` / `-c` | ✓ | ✓ | ✗ | `config/edgion-{type}.yaml` |
| `--log-dir` | ✓ | ✓ | ✗ | Controller: `logs`; Gateway: `/usr/local/edgion/logs` |
| `--log-level` | ✓ | ✓ | ✗ | `info` |
| `--json-format` | ✓ | ✓ | ✗ | `false` |
| `--console` | ✓ | ✗ | ✗ | `true` (Controller only; Gateway console output is config-file-only) |
| `--test-mode` | ✓ | ✗ | ✗ | `false` |

### Controller-specific

| Flag | Default | Description |
|------|--------|------|
| `--grpc-listen` | `0.0.0.0:12151` | gRPC listening address |
| `--admin-listen` | `127.0.0.1:12101` (loopback; set `0.0.0.0` + `allow_admin_ips` for cluster/remote access) | Admin API address |
| `--conf-dir` | — | Configuration directory for FileSystem mode |

### Gateway-specific

| Flag | Default | Description |
|------|--------|------|
| `--server-addr` | (required) | Controller gRPC address |
| `--admin-listen` | `127.0.0.1:12001` | Admin API address |
| `--threads` | CPU core count | Pingora worker threads |
| `--work-stealing` | `true` | Task stealing |
| `--grace-period` | `30s` | Graceful shutdown period |
| `--graceful-shutdown-timeout` | `10s` | Shutdown timeout |
| `--upstream-keepalive-pool-size` | `128` | Upstream connection pool |
| `--integration-testing-mode` | `false` | Integration testing mode |

### ctl-specific

| Flag | Default | Description |
|------|--------|------|
| `--target` / `-t` | `center` | Target API (center/server/client) |
| `--server` | — | Server address (HTTP) |
| `--socket` | — | Reserved argument; currently parsed but ignored, so use HTTP `--server` |
| `--token` | — | Bearer token; overrides `EDGION_TOKEN` and `~/.edgion/token` |
| `--ca-cert` | — | CA PEM file for `https://` servers (env `EDGION_CA_CERT`); replaces the OS CA bundle (`cli::client::ServerTrust`) |
| `-f, --file` | — | File/directory path (apply/delete) |
| `-n, --namespace` | — | Namespace filter |
| `-o, --output` | `table` | Output format (table/json/yaml/wide) |

## Configuration file

### Loading priority (high → low)

```
1. Command-line arguments (CLI flags)
2. Environment variables (EDGION_WORK_DIR)
3. Configuration file (YAML)
4. In-code defaults
```

### Controller configuration example

```yaml
server:
  grpc_listen: "0.0.0.0:12151"
  admin_listen: "127.0.0.1:12101"

logging:
  log_dir: logs
  log_prefix: edgion-controller
  log_level: info
  json_format: false
  console: true

conf_center:
  type: file_system  # or kubernetes
  controller_name: edgion.io/gateway-controller
  conf_dir: edgion-tests/integration/conf

conf_sync:
  default_capacity: 200
  no_sync_kinds: [ReferenceGrant, Secret]

conf_sync_security:
  tls:
    cert: certs/grpc/primary/server.crt
    key: certs/grpc/primary/server.key
    ca: certs/grpc/primary/ca.crt
```

### Gateway configuration example

```yaml
gateway:
  server_addrs: ["http://127.0.0.1:12151"]

logging:
  log_dir: logs
  log_prefix: edgion-gateway
  log_level: "debug,pingora_proxy=error,pingora_core=error"

server:
  threads: 4
  work_stealing: true
  grace_period_seconds: 30
  graceful_shutdown_timeout_seconds: 10

access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log

conf_sync_security:
  active: primary
  certs:
    - name: primary
      cert: certs/grpc/primary/client.crt
      key: certs/grpc/primary/client.key
      ca: certs/grpc/primary/ca.crt
```

The `conf_sync_security` block is mandatory: both binaries refuse to start when it is omitted. Use the canonical, fully commented examples in `config/edgion-controller.yaml` and `config/edgion-gateway.yaml`; Kubernetes deployments use `auto_ca`, while plaintext requires the explicit audited `skip_tls: true` escape hatch on the selected `certs` entry.

## Working directory

### WorkDir layout (`edgion-resources/src/work_dir.rs`)

```
work_dir/
├── logs/              # Log files
├── runtime/           # Runtime state
└── config/            # Configuration files
```

### Initialization flow

1. **Path determination**: CLI `--work-dir` > environment variable `EDGION_WORK_DIR` > config file > default `"."`
2. **Normalization**: empty string or `.` → `std::env::current_dir()` to absolute path; symlinks resolved via `canonicalize()`
3. **Validation and creation**: check directory exists or can be created, test write permission (temporary file), create subdirectories

### Global access

```rust
init_work_dir(base);          // Called once at startup
work_dir();                   // Get the global instance
wd.resolve("logs/app.log");   // Relative path → base/logs/app.log
wd.resolve("/var/log/app.log"); // Absolute path → unchanged
```

## Log initialization

### Shared logging function (`edgion-common/src/observe/logs.rs`)

Both Controller and Gateway call the same `init_logging()` function:

```rust
pub async fn init_logging(config: LogConfig) -> Result<WorkerGuard>
```

**Initialization steps**:
1. Create the log directory
2. Create a file appender (fixed file name, nginx-style; rotation handled by external logrotate)
3. Wrap as a non-blocking appender (background OS thread)
4. Build the tracing subscriber (JSON format or plain text + optional colored console output)
5. Return a `WorkerGuard` (must be kept alive)

### Gateway extra logs

In addition to system logs, the Gateway initializes several dedicated protocol logs:

```rust
init_access_logger(&config.access_log);
init_ssl_logger(&config.ssl_log);
init_tcp_logger(&config.tcp_log);
init_tls_logger(&config.tls_log);
init_udp_logger(&config.udp_log);
```

## Signal handling

### Controller — ShutdownHandle

Location: `edgion-controller/src/conf_mgr/sync_runtime/shutdown.rs`

```rust
pub struct ShutdownHandle {
    inner: Arc<ShutdownController>,
    signal: ShutdownSignal,
}
```

- Uses `tokio::signal` to listen for SIGTERM and SIGINT
- `tokio::select!` waits for either signal then triggers `shutdown()`
- Propagated to ConfMgr and all services for coordinated shutdown

### Gateway — Pingora built-in

The Gateway has no explicit signal handling and relies on Pingora's built-in graceful shutdown:
- `grace_period_seconds` (default 30s)
- `graceful_shutdown_timeout_seconds` (default 10s)

### CLI — short-lived command process

The CLI uses Tokio for asynchronous HTTP and file operations and exits as soon as the command finishes; it does not install the long-running service shutdown flow.

## Global configuration storage

| Global variable | Location | Purpose |
|---------|------|------|
| `CONTROLLER_CONFIG` | `edgion-controller/src/config/mod.rs` | Controller global configuration (`OnceLock`) |
| `GATEWAY_INSTANCE_COUNT` | `edgion-resources/src/rate_limit_local_config.rs` | Gateway cluster instance count (`AtomicU32`, used for cluster-level rate limiting) |
| `GLOBAL_TEST_MODE` | `edgion-common/src/test_mode.rs` | Test mode flag (enables metrics test features) |
| `GLOBAL_INTEGRATION_TESTING_MODE` | `edgion-common/src/test_mode.rs` | Integration testing mode (enables AccessLog storage + metrics test data) |
