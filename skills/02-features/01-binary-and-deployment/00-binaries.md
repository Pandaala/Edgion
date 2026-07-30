---
name: binaries
description: Entry points of the in-repo binaries, full CLI argument reference, work_dir priority rules.
---

# Binary Entry Points and CLI Arguments

## Overview

| Binary | Purpose | Default Ports | Entry |
|--------|---------|---------------|-------|
| `edgion-controller` | Control plane: resource ingestion/validation/processing/distribution | gRPC `:12151`, Admin `:12101`, Probe `:12100`, Metrics `:12190` | `edgion-controller/src/bin/edgion_controller.rs` |
| `edgion-gateway` | Data plane: high-performance proxy (based on Pingora) | Admin `:12001` (loopback, configurable), Probe `:12000` (outward, liveness+readiness), Metrics `:12090` (outward) | `edgion-gateway/src/bin/edgion_gateway.rs` |
| `edgion-cli` | CLI tool: resource CRUD and operations | — | `edgion-cli/src/bin/edgion_ctl.rs` |

> The federation center (`edgion-center`) has been extracted to the separate EdgionCenter repository and is no longer built from this monorepo.

All three binaries follow the **thin entry point** pattern — `src/bin/*.rs` does the minimum amount of work: install the rustls crypto provider → parse arguments → `run()`.

## Working Directory and Relative Paths

```
CLI --work-dir > ENV EDGION_WORK_DIR > YAML config > effective default "."
```

Do not assume that every relative path is resolved against `work_dir`:

| Path | Base |
|------|------|
| `--config-file` | Process current working directory (it is read before `work_dir` is installed) |
| Controller FileSystem `conf_center.conf_dir` / `--conf-dir` | Process current working directory |
| Log paths, conf_sync certificate paths, Gateway Wasm OCI cache path | Effective `work_dir` |

See `docs/en/dev-guide/work-directory.md` before adding another path-valued field.

---

## edgion-controller CLI

```
edgion-controller [OPTIONS]
```

### Argument Schema

| Argument | Short | Type | Default | Description |
|----------|-------|------|---------|-------------|
| `--work-dir` | `-w` | `PathBuf` | Unset (effective `"."`) | Working directory |
| `--config-file` | `-c` | `String` | `config/edgion-controller.yaml` | YAML configuration file path |
| `--conf-dir` | — | `PathBuf` | — | Configuration directory for FileSystem mode (overrides conf_center.conf_dir in YAML) |
| `--fs-poll-interval-ms` | — | `u64` | Auto-calculated | FileSystem watcher poll interval override (minimum 100 ms; testing/tuning) |
| `--test-mode` | — | `bool` | `false` | Enable test mode (metrics testing features) |
| `--grpc-listen` | — | `String` | `0.0.0.0:12151` | gRPC listen address |
| `--admin-listen` | — | `String` | `127.0.0.1:12101` (loopback; set `0.0.0.0` + `allow_admin_ips` for cluster/remote access) | Admin HTTP API address |
| `--probe-listen` | — | `String` | `0.0.0.0:12100` | Liveness/readiness address |
| `--metrics-listen` | — | `String` | `0.0.0.0:12190` | Prometheus metrics address |
| `--allow-admin-ips` | — | comma-separated CIDRs | Empty (allow all) | TCP peer allowlist for Admin API |
| `--log-dir` | — | `String` | `logs` | Log directory |
| `--log-level` | — | `String` | `info` | Log level: trace/debug/info/warn/error |
| `--json-format` | — | `bool` | `false` | JSON-formatted log |
| `--console` | — | `bool` | `true` | Console output |

### Startup Examples

```bash
# FileSystem mode (local development)
edgion-controller -w /opt/edgion --conf-dir /opt/edgion/config/resources

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
| `--work-dir` | `-w` | `PathBuf` | Unset (effective `"."`) | Working directory |
| `--config-file` | `-c` | `String` | `config/edgion-gateway.yaml` | YAML configuration file path |
| `--server-addr` | — | `String` | **Required** | Controller gRPC address (e.g., `http://127.0.0.1:12151`) |
| `--admin-listen` | — | `String` | `127.0.0.1:12001` | Admin API listen address (default: `127.0.0.1:12001`, loopback-only; sensitive endpoints not exposed cluster-wide) |
| `--probe-listen` | — | `String` | `0.0.0.0:12000` | Liveness/readiness probe listen address (serves `/health` and `/ready` only; binds 0.0.0.0 so kubelet can reach the pod IP) |
| `--threads` | — | `usize` | CPU core count | Pingora worker thread count |
| `--work-stealing` | — | `bool` | `true` | Tokio task stealing |
| `--grace-period-seconds` | — | `u64` | `30` | Graceful shutdown wait time in seconds |
| `--graceful-shutdown-timeout-seconds` | — | `u64` | `10` | Shutdown timeout in seconds |
| `--upstream-keepalive-pool-size` | — | `usize` | `128` | Upstream connection pool size |
| `--downstream-keepalive-request-limit` | — | `u32` | `1000` | Maximum requests per downstream connection (0 = unlimited) |
| `--error-log` | — | `String` | — | Pingora error log path |
| `--log-dir` | — | `String` | `logs` | System log directory |
| `--log-level` | — | `String` | `info` | Log level |
| `--json-format` | — | `bool` | `false` | JSON-formatted log |
| `--integration-testing-mode` | — | `bool` | `false` | Integration testing mode (**must not be used in production**). Refused at startup unless `EDGION_ALLOW_INTEGRATION_TESTING_MODE` is also set to a truthy value (`1`/`true`/`yes`/`on`). |
| `--require-fips` | — | `bool` | `false` | Require a FIPS build; exits non-zero if unavailable |
| `--fips-policy[=<auto\|on\|off>]` | — | enum | Unset (effective `auto`) | Downstream approved cipher/curve policy; bare flag means `on` |

### Startup Examples

```bash
# Local-only plaintext development (explicitly insecure)
# config/dev-gateway.yaml must contain: conf_sync_security: { tls: { skip_tls: true } }
edgion-gateway -c config/dev-gateway.yaml --server-addr http://127.0.0.1:12151

# Production: the config must contain matching mTLS or Kubernetes auto-CA settings
edgion-gateway -c config/edgion-gateway.yaml --server-addr https://controller:12151 --threads 8
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
| `--server` | — | `String` | — | Service address (e.g., `http://localhost:12101`) |
| `--socket` | — | `PathBuf` | — | Unix socket path |

### Target Modes

| Target | Connection Port | Operations | Typical Scenario |
|--------|-----------------|------------|------------------|
| `center` | Controller `:12101` | Full CRUD | Resource management (default) |
| `server` | Controller `:12101` | Read-only query | Inspect ServerCache state |
| `client` | Gateway `:12001` | Read-only query | Inspect Gateway-side cache |

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
edgion-cli -t client --server http://localhost:12001 get httproute

# Local-only Proxy-Wasm inspection and strict Envoy v3 conversion
edgion-cli wasm inspect --file plugin.wasm --format json
edgion-cli wasm convert-envoy --file envoy-wasm.yaml --format yaml --report report.json
```

The `wasm` command group short-circuits before token resolution, client
construction, authentication, and network access. `convert-envoy` emits only a
`WasmConfig` fragment; the operator still chooses the EdgionPlugins resource
identity and stage.

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
