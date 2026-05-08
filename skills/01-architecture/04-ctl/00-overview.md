---
name: ctl-overview
description: edgion-cli overview — three target modes (center/server/client), EdgionClient, API routes, debugging usage.
---

# edgion-cli overview

edgion-cli is Edgion's command-line management tool, built on clap. It interacts with Controller and Gateway via HTTP API and supports resource CRUD, cache queries, and operations commands. Its core value is the ability to compare the three-tier cache (ConfCenter → ServerCache → ClientCache) to troubleshoot configuration sync issues.

## Three target modes

| Target | Connection target | Default port | Capability | Data source |
|--------|---------|---------|------|--------|
| **center** (default) | Controller Admin API | :5800 | Full CRUD + reload | ConfCenter (configuration center) |
| **server** | Controller Admin API | :5800 | Read-only query | ServerCache (gRPC server-side cache) |
| **client** | Gateway Admin API | :5900 | Read-only query | ClientCache (gRPC client-side cache) |

```
                    edgion-cli
                   ┌──────────┐
                   │ -t center│──── HTTP :5800 ──► Controller ConfCenter (read/write)
                   │ -t server│──── HTTP :5800 ──► Controller ServerCache (read-only)
                   │ -t client│──── HTTP :5900 ──► Gateway    ClientCache (read-only)
                   └──────────┘
```

### Target vs command capability

| Command | center | server | client |
|------|--------|--------|--------|
| get | Supported | Supported | Supported |
| apply | Supported | Not supported | Not supported |
| delete | Supported | Not supported | Not supported |
| reload | Supported | Not supported | Not supported |

Write operations (apply/delete/reload) are only available in center mode. server and client modes are read-only and used to inspect the state of each cache tier.

## API routes

Different targets correspond to different HTTP API routes:

### center routes (ConfCenter API)

```
GET    /api/v1/namespaced/{kind}              # List resources across all namespaces
GET    /api/v1/namespaced/{kind}/{namespace}   # List resources in the specified namespace
GET    /api/v1/namespaced/{kind}/{ns}/{name}   # Get the specified resource
GET    /api/v1/cluster/{kind}/{name}           # Get a cluster-scoped resource
POST   /api/v1/namespaced/{kind}/{namespace}   # Create a namespaced resource
POST   /api/v1/cluster/{kind}                  # Create a cluster-scoped resource
PUT    /api/v1/namespaced/{kind}/{ns}/{name}   # Update a namespaced resource
PUT    /api/v1/cluster/{kind}/{name}           # Update a cluster-scoped resource
DELETE /api/v1/namespaced/{kind}/{ns}/{name}   # Delete a namespaced resource
DELETE /api/v1/cluster/{kind}/{name}           # Delete a cluster-scoped resource
POST   /api/v1/reload                          # Reload all resources
```

### server routes (ConfigServer cache)

```
GET    /configserver/{kind}/list               # List all resources
GET    /configserver/{kind}?name=N&namespace=NS  # Filter by name/namespace
```

### client routes (ConfigClient cache)

```
GET    /configclient/{kind}/list               # List all resources
GET    /configclient/{kind}?name=N&namespace=NS  # Filter by name/namespace
```

The filtering for server and client routes is done client-side — the full data is fetched first, then filtered inside edgion-cli.

## EdgionClient

The HTTP client implementation; encapsulates the HTTP communication logic with Controller/Gateway.

```rust
pub struct EdgionClient {
    client: Client,         // reqwest HTTP client
    base_url: String,       // Base URL (e.g., http://localhost:5800)
    target: TargetType,     // Target type
    socket_path: Option<PathBuf>,  // Unix socket path (optional)
}
```

### Connection methods

- **HTTP**: the default; the port is chosen based on target type (center/server → 5800, client → 5900)
- **Unix Socket**: specified via the `--socket` argument, suitable for local communication on the same node

### Error handling

EdgionClient provides detailed network-error diagnostics:
- Connection refused → suggests the corresponding component (controller/gateway) may not be running
- DNS resolution failed → suggests checking the hostname
- Timeout → suggests the service may be overloaded
- Generic connection error → suggests checking the server address

## Output formats

| Format | Description |
|------|------|
| `table` (default) | Tabular form, shows key fields |
| `json` | Full JSON output, suitable for programmatic processing |
| `yaml` | Full YAML output, consistent with the configuration file format |
| `wide` | Wide table, shows more fields |

Specified via `-o`: `edgion-cli get httproute -o json`

## Debugging usage

The most valuable use of edgion-cli is troubleshooting configuration sync issues. By comparing the three cache tiers you can quickly locate which layer the issue is in:

### Scenario 1: configuration applied but not in effect on the Gateway

```bash
# 1. Confirm center has been written
edgion-cli get httproute my-route -n default -o yaml

# 2. Confirm the server cache has been updated
edgion-cli -t server get httproute my-route -n default -o yaml

# 3. Confirm the client cache has been synced
edgion-cli -t client get httproute my-route -n default -o yaml
```

- center has it but server doesn't → issue is in the ResourceProcessor processing phase
- server has it but client doesn't → issue is in the gRPC sync phase
- client has it but it's not in effect → issue is in the ConfHandler / preparse phase

### Scenario 2: check sync status

```bash
# Compare the resource counts in server and client
edgion-cli -t server get httproute -o json | jq length
edgion-cli -t client get httproute -o json | jq length
```

## CLI argument overview

| Argument | Short | Description |
|------|--------|------|
| `--target` | `-t` | Target mode: center (default), server, client |
| `--server` | | Server address (e.g., `http://10.0.0.1:5800`) |
| `--socket` | | Unix socket path |
| `--namespace` | `-n` | Namespace filter |
| `--output` | `-o` | Output format: table, json, yaml, wide |
| `--file` | `-f` | File/directory path (for apply/delete) |
| `--dry-run` | | Dry-run mode (apply only) |

## Module structure

```
src/core/ctl/
├── cli/
│   ├── mod.rs              # Cli struct, TargetType, Commands enum, command routing
│   ├── client.rs           # EdgionClient HTTP client implementation
│   ├── output.rs           # OutputFormat, table/JSON/YAML output formatting
│   └── commands/
│       ├── mod.rs          # Subcommand module exports
│       ├── apply.rs        # apply command (YAML parsing → API call)
│       ├── get.rs          # get command (query + formatted output)
│       ├── delete.rs       # delete command
│       └── reload.rs       # reload command
└── mod.rs
```
