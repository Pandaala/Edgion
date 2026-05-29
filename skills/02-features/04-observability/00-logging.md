---
name: logging-features
description: Logging system: Access Log, SSL/TCP/TLS/UDP protocol logs, system log configuration.
---

# Logging System

## Log Categories

| Type | Purpose | Configuration Location | Default Path |
|------|---------|------------------------|--------------|
| System log | Control plane/data plane runtime logs | `logging` | `logs/edgion-{type}.log` |
| Access Log | HTTP/gRPC request log (JSON) | `access_log` | `logs/edgion_access.log` |
| SSL Log | HTTPS connection log | `ssl_log` | `logs/ssl.log` |
| TCP Log | TCP proxy log | `tcp_log` | `logs/tcp_access.log` |
| TLS Log | TLS proxy log | `tls_log` | `logs/tls_access.log` |
| UDP Log | UDP proxy log | `udp_log` | `logs/udp_access.log` |

## Access Log

Access Log is Edgion's core observability mechanism. Every request produces one JSON-formatted log entry, with enough information to diagnose problems.

### Fields

Each entry is serialized from `AccessLogEntry` (`src/core/gateway/observe/access_log/entry.rs`). At a glance the most commonly inspected fields are:

| Field | Description |
|-------|-------------|
| `ts` | Entry timestamp (epoch milliseconds) |
| `host` / `path` | Request host and normalized current path |
| `status` | Response status code |
| `match_info.sv` | Controller → Gateway sync version (omitted when 0) |
| `info` / `errors` | Gateway-internal normal events / exception reasons |
| `stage_logs` | Per-plugin execution log (compact, ≤ 40 bytes per entry) |
| `upstreams` | Upstream connection details (ip, port, timing phases, retry) |
| `request_uri` | **Optional** raw client-entry URI; emitted only when `access_log_options.includeRawUri: true` |

For the complete, authoritative field list (real client IP, trace id, SNI, TLS id, protocol discovery, plugin `PluginLog` format, etc.) see the canonical reference [../../03-coding/observability/00-access-log.md](../../03-coding/observability/00-access-log.md). Do not re-list field names here — that doc tracks `entry.rs` directly.

### Configuration Example

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
      queue_size: 80000
      rotation:
        strategy: daily
        max_files: 7
```

## Protocol Logs

SSL/TCP/TLS/UDP logs share the same configuration schema (see the business log schema section in [../02-config/01-gateway-config.md](../02-config/01-gateway-config.md) for details).

### Enable/Disable

```yaml
tcp_log:
  enabled: true        # Enable TCP log
  output:
    localFile:
      path: logs/tcp_access.log
```

## System Log

The system log uses the Rust `tracing` framework and supports module-level filtering:

```yaml
logging:
  log_level: "info,edgion::core::gateway::routes=debug"  # Debug for a specific module
  json_format: true                                       # Structured JSON
```
