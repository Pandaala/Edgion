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
| `--token` | - | Static CLI token (long-only; `-t` is taken by `--target`) | - |
| `--help` | `-h` | Show help information | - |
| `--version` | `-V` | Show version information | - |

`edgion-cli` is **token-only**: it authenticates to the Controller Admin API with a
single Bearer token (Istio-style). There is no username/password login and no OIDC
in the CLI. The token can be the local admin token or an `edg_`-prefixed `cli_tokens`
token. Token precedence (highest first):

1. `--token <TOKEN>` flag (long-only; `-t` is taken by `--target`)
2. `EDGION_TOKEN` environment variable (avoids exposing the token in shell history)
3. The cached login token at `~/.edgion/token`, written by `edgion-cli login --token`

Validation is lazy: the first request returns `401` (unknown token) or `403`
(permission denied). The only public auth endpoint is `GET /api/v1/auth/status`.

## Target Types

`edgion-cli` supports three target types for connecting to different data sources:

| Target | Component | Default Port | Supported Commands | Description |
|--------|----------|-------------|-------------------|-------------|
| `center` | Controller/ConfCenter | 12101 | get, apply, delete, reload | Full CRUD operations |
| `server` | Controller/ConfigServer | 12101 | get (read-only) | View ConfigServer cache |
| `client` | Gateway/ConfigClient | 12001 | get (read-only) | View Gateway cache |

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
edgion-cli -t client --server http://gateway:12001 get service -n default
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
| edgionconfigdata | Plugin metadata |
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
| `--cert-name` | Name for this certificate set; files go into `<output-dir>/<cert-name>/` | `primary` |
| `--ca-days` | CA certificate validity (days) | `3650` |
| `--cert-days` | Server/client certificate validity (days) | `365` |
| `--server-san` | Additional SAN entries (repeatable) | localhost, 127.0.0.1, ::1 included by default |
| `--force` | Overwrite existing certificates | `false` |

**Generated files** (placed under `<output-dir>/<cert-name>/`):

```
<output-dir>/<cert-name>/
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

After generating certificates, configure mTLS in your YAML config files. See [Control Plane mTLS](infrastructure/mtls.md#control-plane-mtls-conf_sync-grpc) for details.

### gen-token - Generate a Static CLI Token

Generate a new role-scoped CLI token for use with the Controller Admin API.
Does not require a running Controller. The output is the `edg_`-prefixed token
and its SHA-256 hex digest.

```bash
edgion-cli gen-token
```

**Example output:**

```
Token (use with --token or EDGION_TOKEN):
  edg_<URL-safe base64, 43 chars>

SHA-256 (paste into controller cli_tokens[].token_sha256):
  <64 lowercase hex chars>
```

**Workflow:**

1. Run `edgion-cli gen-token` to mint a token.
2. Paste the SHA-256 line into `cli_tokens[].token_sha256` in the Controller
   configuration file.
3. Hot-reload: `edgion-cli reload` (or `POST /api/v1/reload`).
4. Distribute the `edg_...` token to the consumer via a secret manager or
   environment variable — never in plaintext files or shell history.
5. The consumer uses it via `--token edg_...` or `export EDGION_TOKEN=edg_...`.

> **Security:** the SHA-256 is computed over the full `edg_`-prefixed token —
> the exact string sent as the `Authorization: Bearer` value. Never store the raw
> token in the controller config; only the SHA-256 digest goes there.

For RBAC configuration, rotation instructions, and all security notes, see
[CLI Token RBAC](federation-rbac.md#cli-token-rbac).

### login - Persist a Token

Save a Bearer token to `~/.edgion/token` so subsequent commands authenticate
without `--token` or `EDGION_TOKEN`. This is token-only — there is no
username/password or OIDC login.

```bash
edgion-cli login --token <TOKEN>
```

`<TOKEN>` may be the local admin token (retrieve it from the
`edgion-admin-credentials` Secret) or an `edg_`-prefixed `cli_tokens` token from
`gen-token`. Retrieve the admin token from Kubernetes with:

```bash
kubectl -n edgion-system get secret edgion-admin-credentials \
  -o jsonpath='{.data.token}' | base64 -d
```

The cached token is used only when neither `--token` nor `EDGION_TOKEN` is set
(see the precedence list under [Global Options](#global-options)).

## Connection Configuration

### Default Connections

Based on the target type, `edgion-cli` uses the following default connections:

| Target | Default Address |
|--------|----------------|
| center | http://localhost:12101 |
| server | http://localhost:12101 |
| client | http://localhost:12001 |

### Custom Connections

Use the `--server` option to specify a server address:

```bash
# Connect to a remote Controller
edgion-cli --server http://controller.example.com:12101 get httproute

# Connect to a remote Gateway
edgion-cli -t client --server http://gateway.example.com:12001 get service
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
Error: Request to http://localhost:12101/api/v1/namespaced/httproute failed

Connection failed:
  - Is the controller running?
  - Check if the server address is correct
  - Try: curl -v http://localhost:12101/api/v1/namespaced/httproute

Hint: edgion-cli is trying to connect to: http://localhost:12101
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
