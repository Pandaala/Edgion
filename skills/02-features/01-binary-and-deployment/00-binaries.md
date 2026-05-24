---
name: binaries
description: Entry points of the four binaries, full CLI argument reference, work_dir priority rules.
---

# Binary Entry Points and CLI Arguments

## Overview

| Binary | Purpose | Default Ports | Entry |
|--------|---------|---------------|-------|
| `edgion-controller` | Control plane: resource ingestion/validation/processing/distribution | gRPC `:50051`, Admin `:5800` | `src/bin/edgion_controller.rs` |
| `edgion-gateway` | Data plane: high-performance proxy (based on Pingora) | Admin `:5810` (configurable), Metrics `:5901` | `src/bin/edgion_gateway.rs` |
| `edgion-cli` | CLI tool: resource CRUD and operations | — | `src/bin/edgion_ctl.rs` |
| `edgion-center` | Federation center: aggregates resources across federated controllers, serves Center API | — | `src/bin/edgion_center.rs` |

All four binaries follow the **thin entry point** pattern — `src/bin/*.rs` does the minimum amount of work: install the rustls crypto provider → parse arguments → `run()`.

## work_dir Priority

```
CLI --work-dir > ENV EDGION_WORK_DIR > YAML config > Default (".")
```

All relative paths in configuration are resolved against `work_dir`.

---

## edgion-controller CLI

```
edgion-controller [OPTIONS]
```

### Argument Schema

| Argument | Short | Type | Default | Description |
|----------|-------|------|---------|-------------|
| `--work-dir` | `-w` | `PathBuf` | `"."` | Working directory |
| `--config-file` | `-c` | `String` | `config/edgion-controller.yaml` | YAML configuration file path |
| `--conf-dir` | — | `PathBuf` | — | Configuration directory for FileSystem mode (overrides conf_center.conf_dir in TOML) |
| `--test-mode` | — | `bool` | `false` | Enable test mode (metrics testing features) |
| `--grpc-listen` | — | `String` | `0.0.0.0:50051` | gRPC listen address |
| `--admin-listen` | — | `String` | `0.0.0.0:5800` | Admin HTTP API address |
| `--log-dir` | — | `String` | `logs` | Log directory |
| `--log-level` | — | `String` | `info` | Log level: trace/debug/info/warn/error |
| `--json-format` | — | `bool` | `false` | JSON-formatted log |
| `--console` | — | `bool` | `true` | Console output |

### Startup Examples

```bash
# FileSystem mode (local development)
edgion-controller -w /opt/edgion --conf-dir config/resources

# K8s mode (production)
edgion-controller -c config/edgion-controller-k8s.yaml
```

---

## edgion-gateway CLI

```
edgion-gateway [OPTIONS]
```

### Argument Schema

| Argument | Short | Type | Default | Description |
|----------|-------|------|---------|-------------|
| `--work-dir` | `-w` | `PathBuf` | `"."` | Working directory |
| `--config-file` | `-c` | `String` | `config/edgion-gateway.yaml` | YAML configuration file path |
| `--server-addr` | — | `String` | **Required** | Controller gRPC address (e.g., `http://127.0.0.1:50051`) |
| `--admin-listen` | — | `String` | `0.0.0.0:5810` | Admin API listen address (default: `0.0.0.0:5810`) |
| `--threads` | — | `usize` | CPU core count | Pingora worker thread count |
| `--work-stealing` | — | `bool` | `true` | Tokio task stealing |
| `--grace-period` | — | `u64` | `30` | Graceful shutdown wait time in seconds |
| `--graceful-shutdown-timeout` | — | `u64` | `10` | Shutdown timeout in seconds |
| `--upstream-keepalive-pool-size` | — | `usize` | `128` | Upstream connection pool size |
| `--downstream-keepalive-request-limit` | — | `u32` | `1000` | Maximum requests per downstream connection (0 = unlimited) |
| `--error-log` | — | `String` | — | Pingora error log path |
| `--log-dir` | — | `String` | `logs` | System log directory |
| `--log-level` | — | `String` | `info` | Log level |
| `--json-format` | — | `bool` | `false` | JSON-formatted log |
| `--integration-testing-mode` | — | `bool` | `false` | Integration testing mode (**must not be used in production**) |

### Startup Examples

```bash
# Connect to a local Controller
edgion-gateway --server-addr http://127.0.0.1:50051

# Production deployment
edgion-gateway -c config/edgion-gateway.yaml --server-addr http://controller:50051 --threads 8
```

---

## edgion-cli CLI

```
edgion-cli [OPTIONS] <COMMAND>
```

### Global Arguments

| Argument | Short | Type | Default | Description |
|----------|-------|------|---------|-------------|
| `--target` | `-t` | `center\|server\|client` | `center` | Target API |
| `--server` | — | `String` | — | Service address (e.g., `http://localhost:5800`) |
| `--socket` | — | `PathBuf` | — | Unix socket path |

### Target Modes

| Target | Connection Port | Operations | Typical Scenario |
|--------|-----------------|------------|------------------|
| `center` | Controller `:5800` | Full CRUD | Resource management (default) |
| `server` | Controller `:5800` | Read-only query | Inspect ServerCache state |
| `client` | Gateway `:5810` | Read-only query | Inspect Gateway-side cache |

### Subcommands

```bash
# Apply resources
edgion-cli apply -f route.yaml
edgion-cli apply -f config/resources/    # Directory

# Query resources
edgion-cli get httproute                 # List all
edgion-cli get httproute my-route -n default -o yaml

# Delete resources
edgion-cli delete httproute my-route -n default
edgion-cli delete -f route.yaml

# Reload
edgion-cli reload

# Inspect Gateway-side cache
edgion-cli -t client --server http://localhost:5810 get httproute
```

### Subcommand Argument Schemas

**apply**:
| Argument | Short | Type | Description |
|----------|-------|------|-------------|
| `--file` | `-f` | `String` | File or directory path |
| `--dry-run` | — | `bool` | Simulate execution; no actual changes |

**get**:
| Argument | Position | Type | Description |
|----------|----------|------|-------------|
| `kind` | 1st | `String` | Resource type (e.g., httproute, gateway) |
| `name` | 2nd | `String?` | Resource name (omit to list all) |
| `--namespace` / `-n` | — | `String?` | Namespace |
| `--output` / `-o` | — | `String` | Output format: table/json/yaml/wide |

**delete**:
| Argument | Position | Type | Description |
|----------|----------|------|-------------|
| `kind` | 1st | `String?` | Resource type |
| `name` | 2nd | `String?` | Resource name |
| `--namespace` / `-n` | — | `String?` | Namespace |
| `--file` / `-f` | — | `String?` | Delete by file |
