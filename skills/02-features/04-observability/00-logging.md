---
name: logging-features
description: Logging system: Access Log, SSL/TCP/TLS/UDP protocol logs, system log configuration.
---

# Logging System

## Log Categories

| Type | Purpose | Configuration Location | Default Path |
|------|---------|------------------------|--------------|
| System log | Control plane/data plane runtime logs | `[logging]` | `logs/edgion-{type}.log` |
| Access Log | HTTP/gRPC request log (JSON) | `[access_log]` | `logs/edgion_access.log` |
| SSL Log | HTTPS connection log | `[ssl_log]` | `logs/ssl.log` |
| TCP Log | TCP proxy log | `[tcp_log]` | `logs/tcp_access.log` |
| TLS Log | TLS proxy log | `[tls_log]` | `logs/tls_access.log` |
| UDP Log | UDP proxy log | `[udp_log]` | `logs/udp_access.log` |

## Access Log

Access Log is Edgion's core observability mechanism. Every request produces one JSON-formatted log entry, with enough information to diagnose problems.

### Key Fields

| Field | Description |
|-------|-------------|
| `sv` | Server Version (Controller → Gateway sync version) |
| `request_id` | Unique request ID |
| `method` | HTTP method |
| `path` | Request path |
| `status` | Response status code |
| `duration_ms` | Request duration (milliseconds) |
| `upstream_addr` | Backend address |
| `route_name` | Name of the matched route |
| `stage_logs` | Plugin execution log (≤ 40 bytes per plugin) |

### Configuration Example

```toml
[access_log]
enabled = true
[access_log.output.localFile]
path = "logs/edgion_access.log"
queue_size = 80000
[access_log.output.localFile.rotation]
strategy = "daily"
max_files = 7
```

## Protocol Logs

SSL/TCP/TLS/UDP logs share the same configuration schema (see the business log schema section in [../02-config/01-gateway-config.md](../02-config/01-gateway-config.md) for details).

### Enable/Disable

```toml
[tcp_log]
enabled = true      # Enable TCP log
[tcp_log.output.localFile]
path = "logs/tcp_access.log"
```

## System Log

The system log uses the Rust `tracing` framework and supports module-level filtering:

```toml
[logging]
log_level = "info,edgion::core::gateway::routes=debug"  # Debug for a specific module
json_format = true                                        # Structured JSON
```
