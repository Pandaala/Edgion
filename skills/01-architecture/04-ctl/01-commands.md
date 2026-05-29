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
edgion-cli apply -f <file|directory> [--dry-run]
```

### Arguments

| Argument | Description |
|------|------|
| `-f, --file` | YAML file path or a directory containing YAML files |
| `--dry-run` | Validate the configuration only, do not actually write |

### Implementation logic

1. Determine path type: file → single-file processing; directory → traverse all `.yaml`/`.yml` files in the directory
2. Read YAML content, parse `kind`, `metadata.name`, `metadata.namespace`
3. Determine whether the resource already exists:
   - Does not exist → call `POST /api/v1/namespaced/{kind}/{ns}` to create
   - Already exists → call `PUT /api/v1/namespaced/{kind}/{ns}/{name}` to update
4. In dry-run mode, append the query parameter `?dryRun=true` so only validation runs and nothing is written

### Examples

```bash
# Apply a single file
edgion-cli apply -f gateway.yaml

# Apply all YAMLs in a directory
edgion-cli apply -f manifests/

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
3. Re-read all resources from ConfCenter (File/K8s)
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
edgion-cli --server http://10.0.0.1:5800 reload
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
src/core/ctl/
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
| Token generation | `src/core/ctl/cli/token.rs` (`gen_token`) |

## login — authentication

Authenticate with the Controller; the token is saved to a local file and automatically attached by subsequent commands.

### Syntax

```bash
edgion-cli login -u <username> -p <password>
edgion-cli login  # Interactive entry
```

The token is saved to `~/.edgion/token` and automatically attached by subsequent commands.

### Module location

| Module | Path |
|------|------|
| CLI auto-authentication logic | `src/core/ctl/cli/auto_auth.rs` |
| Token file management | `src/core/ctl/cli/token.rs` |

## Auto-authentication (ensure_auth)

Before executing a command, edgion-cli automatically detects authentication status without requiring a manual login:

1. Token already present → use it directly
2. Query `/api/v1/auth/status` → skip if no authentication required
3. Auto-detect the Controller configuration file (`--controller-config` / `EDGION_CONTROLLER_CONFIG` / default paths)
4. Infer conf_dir from the configuration → read the Secret `edgion-admin-credentials` → auto-login
5. Detection failed → prompt to run `edgion-cli login` manually

### Controller vs Center asymmetry

- **Controller**: Step 4 always succeeds — Controller auto-generates the
  `edgion-admin-credentials` Secret at startup, so `ensure_auth` can read it directly.
- **Center**: there is no automatic Secret generation path. Operators must explicitly
  configure `[local_auth]` or `[auth]` in the Center YAML; without it Center returns
  503 for all business routes, the CLI cannot auto-login (manual `edgion-cli login`
  with credentials is required).
  See `skills/02-features/02-config/03-auth-bootstrap.md` for details.

### `/api/v1/auth/status` is always reachable

The `/api/v1/auth/status` endpoint is unconditionally mounted in `compose_admin_routes`
(Controller and Center share the same composition entry; this is the regression
point of fix #01 round 3). Even when Center returns 503 on all business routes
under zero configuration, `/api/v1/auth/status` still returns 200 so the CLI's
`ensure_auth` can query the authentication mode normally.

### Configuration file auto-detection paths

| Priority | Source |
|--------|------|
| 1 | `--controller-config` / `-C` argument |
| 2 | `EDGION_CONTROLLER_CONFIG` environment variable |
| 3 | `config/edgion-controller.yaml` (relative to CWD) |
| 4 | `/usr/local/edgion/config/edgion-controller.yaml` |
| 5 | `/etc/edgion/edgion-controller.yaml` |
