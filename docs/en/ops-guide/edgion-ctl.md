# edgion-ctl Command Line Tool

`edgion-ctl` is Edgion's command line management tool for viewing and managing gateway resource configurations.

## Installation

The compiled binary is located at `target/release/edgion-ctl` or `target/debug/edgion-ctl`.

## Basic Usage

```bash
edgion-ctl [OPTIONS] <COMMAND>
```

### Global Options

| Option | Short | Description | Default |
|--------|-------|-------------|---------|
| `--target` | `-t` | Target API type | `center` |
| `--server` | - | Server address | Auto-selected based on target |
| `--socket` | - | Unix socket path | - |
| `--help` | `-h` | Show help information | - |
| `--version` | `-V` | Show version information | - |

## Target Types

`edgion-ctl` supports three target types for connecting to different data sources:

| Target | Component | Default Port | Supported Commands | Description |
|--------|----------|-------------|-------------------|-------------|
| `center` | Controller/ConfCenter | 5800 | get, apply, delete, reload | Full CRUD operations |
| `server` | Controller/ConfigServer | 5800 | get (read-only) | View ConfigServer cache |
| `client` | Gateway/ConfigClient | 5900 | get (read-only) | View Gateway cache |

### Usage Examples

```bash
# center (default) - full functionality
edgion-ctl get httproute
edgion-ctl apply -f route.yaml
edgion-ctl delete httproute my-route -n default

# server - view Controller's ConfigServer cache
edgion-ctl -t server get httproute
edgion-ctl -t server get httproute -n prod

# client - view Gateway's ConfigClient cache
edgion-ctl -t client get httproute
edgion-ctl -t client --server http://gateway:5900 get service -n default
```

## Command Reference

### get - Retrieve Resources

Get a single resource or list resources.

```bash
edgion-ctl get <KIND> [NAME] [OPTIONS]
```

**Parameters:**
- `KIND`: Resource type (e.g., httproute, service, gateway)
- `NAME`: Resource name (optional; lists all if not specified)

**Options:**
- `-n, --namespace <NS>`: Specify namespace
- `-o, --output <FORMAT>`: Output format (table, json, yaml, wide)

**Examples:**

```bash
# List all HTTPRoutes
edgion-ctl get httproute

# List HTTPRoutes in a specific namespace
edgion-ctl get httproute -n production

# Get a specific resource in YAML format
edgion-ctl get httproute my-route -n default -o yaml

# Get a specific resource in JSON format
edgion-ctl get service backend-svc -n default -o json
```

**Supported resource types:**

| Type | Description |
|------|-------------|
| httproute | HTTP routing rules |
| grpcroute | gRPC routing rules |
| tcproute | TCP routing rules |
| udproute | UDP routing rules |
| tlsroute | TLS routing rules |
| service | Kubernetes Service |
| endpointslice | EndpointSlice |
| endpoint | Endpoints |
| gateway | Gateway resource |
| gatewayclass | GatewayClass resource |
| edgiontls | Edgion TLS configuration |
| edgionplugins | Edgion HTTP plugin configuration |
| edgionstreamplugins | Edgion Stream plugin configuration |
| pluginmetadata | Plugin metadata |
| linksys | LinkSys configuration |
| referencegrant | ReferenceGrant |
| backendtlspolicy | BackendTLSPolicy |
| edgiongatewayconfig | Edgion Gateway configuration |

### apply - Apply Configuration

Create or update resources from a YAML file. **Only supported with the center target.**

```bash
edgion-ctl apply -f <FILE|DIR> [OPTIONS]
```

**Options:**
- `-f, --file <PATH>`: YAML file or directory path (required)
- `--dry-run`: Dry run, do not actually apply

**Examples:**

```bash
# Apply a single file
edgion-ctl apply -f route.yaml

# Apply all YAML files in a directory
edgion-ctl apply -f ./configs/

# Dry run
edgion-ctl apply -f route.yaml --dry-run
```

### delete - Delete Resources

Delete a specified resource. **Only supported with the center target.**

```bash
edgion-ctl delete <KIND> <NAME> [OPTIONS]
edgion-ctl delete -f <FILE>
```

**Options:**
- `-n, --namespace <NS>`: Specify namespace
- `-f, --file <PATH>`: Read resources to delete from a YAML file

**Examples:**

```bash
# Delete a specific resource
edgion-ctl delete httproute my-route -n default

# Delete from file
edgion-ctl delete -f route.yaml
```

### reload - Reload

Reload all resources from storage. **Only supported with the center target.**

```bash
edgion-ctl reload
```

## Connection Configuration

### Default Connections

Based on the target type, `edgion-ctl` uses the following default connections:

| Target | Default Address |
|--------|----------------|
| center | http://localhost:5800 |
| server | http://localhost:5800 |
| client | http://localhost:5900 |

### Custom Connections

Use the `--server` option to specify a server address:

```bash
# Connect to a remote Controller
edgion-ctl --server http://controller.example.com:5800 get httproute

# Connect to a remote Gateway
edgion-ctl -t client --server http://gateway.example.com:5900 get service
```

## Output Formats

### table (Default)

Displays resource lists in table format:

```
┌──────────────┬───────────┬───────────┐
│ NAME         │ NAMESPACE │ KIND      │
├──────────────┼───────────┼───────────┤
│ my-route     │ default   │ HTTPRoute │
│ api-route    │ prod      │ HTTPRoute │
└──────────────┴───────────┴───────────┘
```

### json

Outputs complete resource information in JSON format.

### yaml

Outputs complete resource information in YAML format.

### wide

Extended table display with additional fields.

## Troubleshooting

### Connection Failure

If the connection fails, `edgion-ctl` displays detailed error information and hints:

```
Error: Request to http://localhost:5800/api/v1/namespaced/httproute failed

Connection failed:
  - Is the controller running?
  - Check if the server address is correct
  - Try: curl -v http://localhost:5800/api/v1/namespaced/httproute

Hint: edgion-ctl is trying to connect to: http://localhost:5800
      Target: Center (controller)
      Use --server to specify a different address
```

### Common Issues

1. **"apply command only supported for 'center' target"**
   
   The `apply`, `delete`, and `reload` commands can only be used with the `center` target. The `server` and `client` targets support read-only operations only.

2. **Resource not found**
   
   Check that the resource name, namespace, and target point to the correct component.

3. **Connection timeout**
   
   Confirm the target service is running and network connectivity is normal.

## Comparison with kubectl

| Operation | kubectl | edgion-ctl |
|-----------|---------|------------|
| Get resources | `kubectl get httproute` | `edgion-ctl get httproute` |
| Specify namespace | `kubectl -n prod get httproute` | `edgion-ctl get httproute -n prod` |
| Apply config | `kubectl apply -f route.yaml` | `edgion-ctl apply -f route.yaml` |
| Delete resource | `kubectl delete httproute my-route` | `edgion-ctl delete httproute my-route` |
| Output YAML | `kubectl get httproute -o yaml` | `edgion-ctl get httproute -o yaml` |

**Difference:** `edgion-ctl` can connect directly to Edgion's Admin API without requiring a Kubernetes cluster, making it suitable for file-system mode deployments.
