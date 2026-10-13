---
name: ctl-overview
description: edgion-cli overview — three target modes (center/server/client), EdgionClient, API routes, debugging usage.
---

# edgion-cli overview

edgion-cli is Edgion's command-line management tool, built on clap. It interacts with Controller and Gateway via HTTP API and supports resource CRUD, cache queries, and operations commands. Its core value is the ability to compare the three-tier cache (ConfCenter → ServerCache → ClientCache) to troubleshoot configuration sync issues.

The CLI target name `center` refers to the Controller's ConfCenter data source. It does not connect to the external federation Center binary.

## Three target modes

| Target | Connection target | Default port | Capability | Data source |
|--------|---------|---------|------|--------|
| **center** (default) | Controller Admin API | :12101 | Full CRUD + reload | ConfCenter (configuration center) |
| **server** | Controller Admin API | :12101 | Read-only query | ServerCache (gRPC server-side cache) |
| **client** | Gateway Admin API | :12001 | Read-only query | ClientCache (gRPC client-side cache) |

```
                    edgion-cli
                   ┌──────────┐
                   │ -t center│──── HTTP :12101 ──► Controller ConfCenter (read/write)
                   │ -t server│──── HTTP :12101 ──► Controller ServerCache (read-only)
                   │ -t client│──── HTTP :12001 ──► Gateway    ClientCache (read-only)
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
GET    /api/v1/cluster/{kind}                  # List cluster-scoped resources
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

For server/client list requests, namespace-only filtering is applied by the CLI after fetching the list. Named requests use the query endpoint and are filtered by the server.

Single-page and `--all` cache queries share the same command-owned namespace
selection (ARCH-176). Each page is filtered before Table/Wide output or JSON/YAML
accumulation; the original server continuation token remains authoritative even
when no rows match. Table headers follow the first nonempty printed page, not the
first fetched page. Center's namespaced and metadata-key endpoints are unchanged.

## EdgionClient

### Paginated output ownership

For `get --all` Table/Wide output, `print_resource_list_page_streaming` borrows
the parsed page's resource array while projecting owned metadata rows. It does
not clone unused spec/status trees (ARCH-171). The caller retains the page for
continuation-token extraction. JSON/YAML accumulation keeps its separate owned
lifetime across pages. Row formatting, empty-page behavior and continuation
headers are unchanged; this is not a measured latency or process-memory bound.


The HTTP client implementation; encapsulates the HTTP communication logic with Controller/Gateway.

```rust
pub struct EdgionClient {
    client: Client,         // reqwest HTTP client
    base_url: String,       // Base URL (e.g., http://localhost:12101)
    target: TargetType,     // Target type
    token: Option<Zeroizing<String>>, // Optional static bearer token
}
```

### Connection methods

- **HTTP**: the implemented transport; the port is chosen based on target type (center/server → 12101, client → 12001)
- **Reserved socket flag**: `--socket` is currently parsed but ignored by `EdgionClient`; do not document or rely on Unix-socket connectivity until the client implements it

### Error handling

Connection diagnostics share `cli::client::diagnostic_origin` (ARCH-178): only the HTTP(S)
scheme, host and port are displayed, excluding userinfo, path, query and fragment.
Malformed or non-HTTP(S) inputs use a fixed placeholder. Request and auth-probe
errors remove reqwest's attached URL before rendering, including nested error
chains. Get hints and best-effort login warnings use the same projection; actual
request URLs, bearer authentication and token persistence are unchanged.

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
| `--server` | | Server address (e.g., `http://10.0.0.1:12101`) |
| `--socket` | | Reserved; currently ignored |
| `--token` | | Bearer token; overrides the environment and token file |
| `--namespace` | `-n` | `get` / `delete` namespace filter |
| `--output` | `-o` | `get` format: table, json, yaml, wide |
| `--file` | `-f` | `apply` / `delete` file or directory path |
| `--dry-run` | | `apply` local preview; parses metadata and sends no request |
| `--metadata-only` | | `get` via the ConfCenter metadata-key endpoint; center target only |
| `--limit`, `--continue`, `--all` | | `get` pagination controls |

## Module structure

```
edgion-cli/src/
├── cli/
│   ├── mod.rs              # Cli struct, TargetType, Commands enum, command routing
│   ├── auto_auth.rs        # Bearer-token precedence resolution
│   ├── client.rs           # EdgionClient HTTP client implementation
│   ├── output.rs           # OutputFormat, table/JSON/YAML output formatting
│   ├── token.rs            # Token generation and local persistence
│   └── commands/
│       ├── mod.rs          # Subcommand module exports
│       ├── apply.rs        # apply command (YAML parsing → API call)
│       ├── get.rs          # get command (query + formatted output)
│       ├── delete.rs       # delete command
│       ├── reload.rs       # reload command
│       ├── gen_certs.rs    # gen-certs command
│       └── login.rs        # token persistence command
└── lib.rs
```
