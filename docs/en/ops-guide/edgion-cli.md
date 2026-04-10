# edgion-cli Command Line Tool

`edgion-cli` is Edgion's command line management tool for viewing and managing gateway resource configurations.

## Installation

The compiled binary is located at `target/release/edgion-cli` or `target/debug/edgion-cli`.

## Basic Usage

```bash
edgion-cli [OPTIONS] <COMMAND>
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

`edgion-cli` supports three target types for connecting to different data sources:

| Target | Component | Default Port | Supported Commands | Description |
|--------|----------|-------------|-------------------|-------------|
| `center` | Controller/ConfCenter | 5800 | get, apply, delete, reload | Full CRUD operations |
| `server` | Controller/ConfigServer | 5800 | get (read-only) | View ConfigServer cache |
| `client` | Gateway/ConfigClient | 5900 | get (read-only) | View Gateway cache |

### Usage Examples

```bash
# center (default) - full functionality
edgion-cli get httproute
edgion-cli apply -f route.yaml
edgion-cli delete httproute my-route -n default

# server - view Controller's ConfigServer cache
edgion-cli -t server get httproute
edgion-cli -t server get httproute -n prod

# client - view Gateway's ConfigClient cache
edgion-cli -t client get httproute
edgion-cli -t client --server http://gateway:5900 get service -n default
```

## Command Reference

### get - Retrieve Resources

Get a single resource or list resources.

```bash
edgion-cli get <KIND> [NAME] [OPTIONS]
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
edgion-cli get httproute

# List HTTPRoutes in a specific namespace
edgion-cli get httproute -n production

# Get a specific resource in YAML format
edgion-cli get httproute my-route -n default -o yaml

# Get a specific resource in JSON format
edgion-cli get service backend-svc -n default -o json
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
edgion-cli apply -f <FILE|DIR> [OPTIONS]
```

**Options:**
- `-f, --file <PATH>`: YAML file or directory path (required)
- `--dry-run`: Dry run, do not actually apply

**Examples:**

```bash
# Apply a single file
edgion-cli apply -f route.yaml

# Apply all YAML files in a directory
edgion-cli apply -f ./configs/

# Dry run
edgion-cli apply -f route.yaml --dry-run
```

### delete - Delete Resources

Delete a specified resource. **Only supported with the center target.**

```bash
edgion-cli delete <KIND> <NAME> [OPTIONS]
edgion-cli delete -f <FILE>
```

**Options:**
- `-n, --namespace <NS>`: Specify namespace
- `-f, --file <PATH>`: Read resources to delete from a YAML file

**Examples:**

```bash
# Delete a specific resource
edgion-cli delete httproute my-route -n default

# Delete from file
edgion-cli delete -f route.yaml
```

### reload - Reload

Reload all resources from storage. **Only supported with the center target.**

```bash
edgion-cli reload
```

### gen-certs - Generate mTLS Certificates

Generate a self-signed certificate bundle for securing the conf_sync gRPC channel with mTLS. This command does not require a running Controller or Gateway.

```bash
edgion-cli gen-certs [OPTIONS]
```

**Options:**

| Option | Description | Default |
|--------|-------------|---------|
| `--output` | Output target: `file` or `k8s` | `file` |
| `--output-dir` | Output directory (for `--output file`) | `certs/grpc/` |
| `--ca-days` | CA certificate validity (days) | `3650` |
| `--cert-days` | Server/client certificate validity (days) | `365` |
| `--server-san` | Additional SAN entries (repeatable) | localhost, 127.0.0.1, ::1 included by default |
| `--force` | Overwrite existing certificates | `false` |

**Generated files:**

```
<output-dir>/
├── ca.crt        # CA certificate
├── ca.key        # CA private key (for signing only)
├── server.crt    # Controller server certificate
├── server.key    # Controller server private key
├── client.crt    # Gateway client certificate
└── client.key    # Gateway client private key
```

**Examples:**

```bash
# Generate to default directory
edgion-cli gen-certs --output file

# Custom output directory with force overwrite
edgion-cli gen-certs --output file --output-dir /etc/edgion/certs --force

# Add K8s service FQDN as SAN
edgion-cli gen-certs --output file \
    --server-san edgion-controller.edgion-system.svc.cluster.local

# Generate as K8s Secrets
edgion-cli gen-certs --output k8s
```

After generating certificates, configure mTLS in your TOML files. See [Control Plane mTLS](infrastructure/mtls.md#control-plane-mtls-conf_sync-grpc) for details.

## Connection Configuration

### Default Connections

Based on the target type, `edgion-cli` uses the following default connections:

| Target | Default Address |
|--------|----------------|
| center | http://localhost:5800 |
| server | http://localhost:5800 |
| client | http://localhost:5900 |

### Custom Connections

Use the `--server` option to specify a server address:

```bash
# Connect to a remote Controller
edgion-cli --server http://controller.example.com:5800 get httproute

# Connect to a remote Gateway
edgion-cli -t client --server http://gateway.example.com:5900 get service
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

If the connection fails, `edgion-cli` displays detailed error information and hints:

```
Error: Request to http://localhost:5800/api/v1/namespaced/httproute failed

Connection failed:
  - Is the controller running?
  - Check if the server address is correct
  - Try: curl -v http://localhost:5800/api/v1/namespaced/httproute

Hint: edgion-cli is trying to connect to: http://localhost:5800
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

| Operation | kubectl | edgion-cli |
|-----------|---------|------------|
| Get resources | `kubectl get httproute` | `edgion-cli get httproute` |
| Specify namespace | `kubectl -n prod get httproute` | `edgion-cli get httproute -n prod` |
| Apply config | `kubectl apply -f route.yaml` | `edgion-cli apply -f route.yaml` |
| Delete resource | `kubectl delete httproute my-route` | `edgion-cli delete httproute my-route` |
| Output YAML | `kubectl get httproute -o yaml` | `edgion-cli get httproute -o yaml` |

**Difference:** `edgion-cli` can connect directly to Edgion's Admin API without requiring a Kubernetes cluster, making it suitable for file-system mode deployments.

## Related Documentation

- [Gateway Resource Overview](gateway/overview.md)
- [Monitoring Metrics](observability/metrics.md)
- [Access Log User Guide](observability/access-log.md)
- [Control Plane mTLS](infrastructure/mtls.md)
