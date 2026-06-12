---
name: cli-and-startup
description: Command-line conventions, working-directory layout, config-file path conventions, log initialization, and signal handling shared by all four binaries.
---

# Unified command-line and configuration conventions

## Entry-point pattern

All four binaries follow the **thin entry point** pattern — `src/bin/*.rs` does the minimum amount of work:

| bin | Entry logic |
|-----|---------|
| edgion-controller | Install rustls crypto provider → `parse_args()` → `run()` |
| edgion-gateway | Install rustls crypto provider → `parse_args()` → `run()` (plain `fn main()`, manages its own Tokio runtime) |
| edgion-cli | `Cli::parse()` → `cli.run().await` (single-threaded Tokio) |

## Command-line arguments

### Shared flags

| Flag | Controller | Gateway | ctl | Default |
|------|:---------:|:-------:|:---:|--------|
| `--work-dir` / `-w` | ✓ | ✓ | ✗ | `"."` |
| `--config-file` / `-c` | ✓ | ✓ | ✗ | `config/edgion-{type}.yaml` |
| `--log-dir` | ✓ | ✓ | ✗ | `logs` |
| `--log-level` | ✓ | ✓ | ✗ | `info` |
| `--json-format` | ✓ | ✓ | ✗ | `false` |
| `--console` | ✓ | ✓ | ✗ | `true` |
| `--test-mode` | ✓ | ✗ | ✗ | `false` |

### Controller-specific

| Flag | Default | Description |
|------|--------|------|
| `--grpc-listen` | `0.0.0.0:12151` | gRPC listening address |
| `--admin-listen` | `0.0.0.0:12101` | Admin API address |
| `--conf-dir` | — | Configuration directory for FileSystem mode |

### Gateway-specific

| Flag | Default | Description |
|------|--------|------|
| `--server-addr` | (required) | Controller gRPC address |
| `--admin-listen` | — | Admin API address |
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
| `--socket` | — | Unix socket path |
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
  admin_listen: "0.0.0.0:12101"

logging:
  log_dir: logs
  log_prefix: edgion-controller
  log_level: info
  json_format: false
  console: true

conf_center:
  type: file_system  # or kubernetes
  conf_dir: examples/test/conf

conf_sync:
  default_capacity: 200
  no_sync_kinds: [ReferenceGrant, Secret]
```

### Gateway configuration example

```yaml
gateway:
  server_addr: "http://127.0.0.1:12151"

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
```

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

### ctl — not needed

ctl is a synchronous CLI tool that exits as soon as the command finishes.

## Global configuration storage

| Global variable | Location | Purpose |
|---------|------|------|
| `CONTROLLER_CONFIG` | `edgion-common/src/config/mod.rs` | Controller global configuration (`OnceLock`) |
| `GATEWAY_INSTANCE_COUNT` | same as above | Gateway cluster instance count (`AtomicU32`, used for cluster-level rate limiting) |
| `GLOBAL_TEST_MODE` | same as above | Test mode flag (enables metrics test features) |
| `GLOBAL_INTEGRATION_TESTING_MODE` | same as above | Integration testing mode (enables AccessLog storage + metrics test data) |
