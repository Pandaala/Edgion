---
name: ctl-commands
description: edgion-cli subcommand details — apply, get, delete, reload arguments, implementation logic, examples.
---

# edgion-cli subcommands

edgion-cli provides 7 subcommands: apply (apply configuration), get (query resources), delete (delete resources), reload (reload resources), gen-certs (generate mTLS certificates), gen-token (mint a CLI token), and login (authentication). apply, delete, and reload are only available with the center target; gen-certs and gen-token do not depend on a target and do not contact the controller.

## apply

Apply configuration resources from a file or directory; only supports the center target.

### Syntax

```bash
edgion-cli apply -f <file|directory> [-f <file|directory> ...] [--dry-run]
```

### Arguments

| Argument | Description |
|------|------|
| `-f, --file` | YAML file path or a directory containing YAML files; repeatable and processed in command-line order using one client |
| `--dry-run` | Local preview: parse resource metadata and print the intended operation without contacting the Controller |

### Implementation logic

1. Process each `-f` path in command-line order using one CLI runtime and HTTP client
2. Determine path type: file → single-file processing; directory → traverse all `.yaml`/`.yml` files in the directory
3. Split multi-document YAML streams and parse `kind`, `metadata.name`, and `metadata.namespace` from each document
4. In `--dry-run` mode, print `Would apply ...` and return without any network request; this is not schema validation
5. Otherwise determine whether the resource already exists:
   - Does not exist → call `POST /api/v1/namespaced/{kind}/{ns}` to create
   - Already exists → call `PUT /api/v1/namespaced/{kind}/{ns}/{name}` to update

### Examples

```bash
# Apply a single file
edgion-cli apply -f gateway.yaml

# Apply all YAMLs in a directory
edgion-cli apply -f manifests/

# Apply an ordered batch while reusing one client
edgion-cli apply -f gateway.yaml -f backend.yaml -f route.yaml

# Dry-run validation
edgion-cli apply -f gateway.yaml --dry-run
```

## get

Query resources; supports all three target modes.

### Syntax

```bash
edgion-cli [-t <target>] get <kind> [name] [-n <namespace>] [-o <format>]
```

### Arguments

| Argument | Description |
|------|------|
| `kind` | Resource type (e.g., httproute, gateway, service); case-insensitive |
| `name` | Resource name (optional; lists all when omitted) |
| `-n, --namespace` | Namespace filter |
| `-o, --output` | Output format: table (default), json, yaml, wide |
| `--metadata-only` | Use the ConfCenter metadata-key endpoint; valid only with the center target |
| `--limit` | Maximum items requested per page |
| `--continue` | Continue token returned by the previous page; conflicts with `--all` |
| `--all` | Auto-paginate and concatenate all pages |

### Query logic

The query path differs based on the target:

| Target | With name | Without name |
|--------|---------|---------|
| center | `GET /api/v1/namespaced/{kind}/{ns}/{name}` or `GET /api/v1/cluster/{kind}/{name}` | `GET /api/v1/namespaced/{kind}` or filter by namespace |
| server | `GET /configserver/{kind}?name=N&namespace=NS` | `GET /configserver/{kind}/list` |
| client | `GET /configclient/{kind}?name=N&namespace=NS` | `GET /configclient/{kind}/list` |

In server/client modes, namespace filtering is done client-side: the full data is fetched first, then filtered by namespace.

### Examples

```bash
# List all HTTPRoutes (center mode)
edgion-cli get httproute

# Filter by namespace
edgion-cli get httproute -n production

# Get the YAML of a specific resource
edgion-cli get httproute my-route -n default -o yaml

# View resources in the Gateway cache (JSON format)
edgion-cli -t client get httproute -o json

# Wide table to show more information
edgion-cli get gateway -o wide
```

## delete

Delete resources; only supports the center target. Two delete modes are supported: by kind/name, or from a file.

### Syntax

```bash
# Mode 1: delete by kind/name
edgion-cli delete <kind> <name> [-n <namespace>]

# Mode 2: delete from a file
edgion-cli delete -f <file>
```

### Arguments

| Argument | Description |
|------|------|
| `kind` | Resource type (mutually exclusive with -f) |
| `name` | Resource name (mutually exclusive with -f) |
| `-n, --namespace` | Namespace |
| `-f, --file` | Read resources to delete from a YAML file |

### Implementation logic

- **Delete by kind/name**: directly call `DELETE /api/v1/namespaced/{kind}/{ns}/{name}` or `DELETE /api/v1/cluster/{kind}/{name}`
- **Delete from file**: parse `kind`, `metadata.name`, `metadata.namespace` from the YAML file and call the corresponding DELETE API

### Examples

```bash
# Delete by kind/name
edgion-cli delete httproute my-route -n default

# Delete from file
edgion-cli delete -f gateway.yaml
```

## reload

Reload all resources from storage; only supports the center target. Triggers a full reprocessing on the Controller.

### Syntax

```bash
edgion-cli reload
```

### Implementation logic

Calls `POST /api/v1/reload`. After the Controller receives the request:

1. Clear all ServerCaches (`reset_for_relink`)
2. Generate a new `server_id`
3. Re-read all resources from ConfCenter (File/K8s/Etcd)
4. Re-run the full processing pipeline
5. Gateway detects the `server_id` change and automatically re-runs a full relist

### Use cases

- Forcing a re-sync when Controller is out of state with external storage
- Manually triggering full reprocessing in development
- Troubleshooting resource-processing pipeline issues

### Examples

```bash
# Reload all resources
edgion-cli reload

# Connect to a Controller at a specific address
edgion-cli --server http://10.0.0.1:12101 reload
```

## gen-certs

Generate the certificate bundle (CA + Server + Client) needed for conf_sync gRPC mTLS, implemented with the rcgen library. This command does not depend on a target and does not require connecting to any service.

### Syntax

```bash
edgion-cli gen-certs [OPTIONS]
```

### Arguments

| Argument | Description | Default |
|------|------|------|
| `--output` | Output target: `file` (write to files) or `k8s` (write to K8s Secrets) | `file` |
| `--output-dir` | File output directory (only effective with `--output file`) | `certs/grpc/` |
| `--cert-name` | Certificate set name; files are written to `<output-dir>/<cert-name>/`, K8s Secret names include this suffix | `primary` |
| `--ca-days` | CA certificate validity in days | `3650` (10 years) |
| `--cert-days` | Server/client certificate validity in days | `365` |
| `--server-san` | Additional SANs (may be specified multiple times) | Defaults include `localhost`, `127.0.0.1`, `::1` |
| `--force` | Force overwrite of existing certificates | `false` |

### Generated files

Each invocation writes to the `<output-dir>/<cert-name>/` subdirectory; different `--cert-name` values are independent of each other:

```
<output-dir>/<cert-name>/
├── ca.crt            # CA certificate (distributed to both sides for peer verification)
├── ca.key            # CA private key (signing only; do not deploy to runtime)
├── server.crt        # Controller server certificate
├── server.key        # Controller server private key
├── client.crt        # Gateway client certificate
└── client.key        # Gateway client private key
```

### Certificate properties

| Certificate | CN | SAN | EKU |
|------|----|----|-----|
| CA | edgion-grpc-ca | — | — |
| Server | edgion-controller | localhost, 127.0.0.1, ::1, + `--server-san` | ServerAuth |
| Client | edgion-gateway | — | ClientAuth |

### Implementation logic

1. Use rcgen to create a self-signed CA (ECDSA P-256)
2. Use the CA to sign the Server certificate (with SANs and ServerAuth EKU)
3. Use the CA to sign the Client certificate (with ClientAuth EKU)
4. `--output file`: write PEM files to `<output-dir>/<cert-name>/`
5. `--output k8s`: write K8s Secrets, with names of the form `{prefix}-controller-tls-{cert-name}` / `{prefix}-gateway-tls-{cert-name}`

### Use cases

- Local development/testing: quickly generate self-signed certificates to enable mTLS
- CI/CD: generate certificates automatically in integration tests
- Production initialization: generate once and hand off to a certificate management system for rotation
- Certificate-rotation pre-staging: generate the secondary set and pre-write configuration; switching only requires changing the `active` field

### Examples

```bash
# Generate the primary certificate set (default name)
edgion-cli gen-certs --output file --output-dir certs/grpc/

# Explicit name; equivalent to the previous one
edgion-cli gen-certs --output file --output-dir certs/grpc/ --cert-name primary

# Pre-stage a secondary certificate set (for rotation)
edgion-cli gen-certs --output file --output-dir certs/grpc/ --cert-name secondary \
    --server-san edgion-controller.edgion-system.svc.cluster.local

# Specify output directory and force overwrite
edgion-cli gen-certs --output file --output-dir /etc/edgion/certs --cert-name primary --force

# Write to K8s Secrets (names: {prefix}-controller-tls-primary, etc.)
edgion-cli gen-certs --output k8s --cert-name primary
```

### Module structure

```
edgion-cli/src/
├── certs/
│   └── mod.rs                # generate_certs() → CertBundle (CA, Server, Client)
└── cli/
    └── commands/
        └── gen_certs.rs      # GenCertsArgs, write_to_files(), write_to_k8s_secrets()
```

## gen-token

Mint a new CLI token and print it together with its SHA-256 digest. Does not contact the controller.

### Syntax

```bash
edgion-cli gen-token
```

### Implementation logic

1. Generate 32 CSPRNG bytes, encode as URL-safe base64 (no padding), and prepend an `edg_` prefix to form the token.
2. Compute the SHA-256 over the **full `edg_`-prefixed token** (the exact `Authorization: Bearer <token>` string the CLI sends).
3. Print the token (used with `--token` or `EDGION_TOKEN`) and the SHA-256 (pasted into the controller `cli_tokens[].token_sha256`).

### Module location

| Module | Path |
|------|------|
| Token generation | `edgion-cli/src/cli/token.rs` (`gen_token`) |

## login — persist a token

Authentication is **token-only** (Istio-style pre-shared bearer token). `login` simply persists the supplied token to `~/.edgion/token` (mode 0600) so subsequent commands pick it up automatically. There is no username/password exchange and no OIDC.

### Syntax

```bash
edgion-cli login --token <edg_…>
```

`--token` is required (omitting it is a parse error). The token is saved to `~/.edgion/token`; the command optionally probes `/api/v1/auth/status` for a friendly warning but persists the token either way. To mint a token, use `edgion-cli gen-token`.

### Module location

| Module | Path |
|------|------|
| Token resolution (`resolve_token`) | `edgion-cli/src/cli/auto_auth.rs` |
| Token file management | `edgion-cli/src/cli/token.rs` |
| `login` command | `edgion-cli/src/cli/commands/login.rs` |

## Token resolution (resolve_token)

Before each request, edgion-cli resolves the bearer token to attach. There is no auto-login, no `/api/v1/auth/me` probe, and no Secret reading — just a fixed precedence (highest first):

1. `--token` command-line flag (if supplied and non-empty)
2. `EDGION_TOKEN` environment variable
3. The cached token file at `~/.edgion/token`

Empty values are treated as absent at every level; the token is wrapped in `Zeroizing` so its buffer is wiped on drop. If no token is found, the request is sent without an `Authorization` header (the controller will then `403`, or `503` if it has no token configured).

### `/api/v1/auth/status` is always reachable

The public `GET /api/v1/auth/status` endpoint is mounted outside the authz `route_layer`, so it returns `200` even when the controller has no token configured (reporting `status: Initializing`). This lets `edgion-cli login` probe the controller for a friendly warning without ever blocking token persistence.
