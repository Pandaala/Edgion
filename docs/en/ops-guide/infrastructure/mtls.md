# mTLS Configuration

> **🔌 Edgion Extension**
> 
> mTLS configuration is implemented through the `EdgionTls` CRD. This is an Edgion extension feature, not part of the standard Gateway API specification.

Configure Mutual TLS (mTLS) authentication.

## Overview

mTLS requires both the client and server to verify each other's certificates:

```
Client [cert] <-> [verify] Gateway [cert] <-> [verify] Client
```

## Configuration

Use the EdgionTls resource to configure mTLS:

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: mtls-config
spec:
  secretRef:
    name: server-tls
  clientAuth:
    mode: Require  # Require client certificate
    caCertificateRefs:
      - name: client-ca
```

## Configuration Reference

### clientAuth

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| mode | String | | None/Request/Require |
| caCertificateRefs | array | | Client CA certificates |

### clientAuth.mode

| Mode | Description |
|------|-------------|
| None | Do not verify client certificate (default) |
| Request | Request client certificate but do not enforce |
| Require | Mandatory client certificate |

## CA Certificate Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: client-ca
type: Opaque
data:
  ca.crt: <base64-encoded-ca-cert>
```

## Examples

### Example 1: Mandatory mTLS

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: strict-mtls
spec:
  secretRef:
    name: server-tls
  clientAuth:
    mode: Require
    caCertificateRefs:
      - name: trusted-client-ca
---
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: mtls-gateway
spec:
  gatewayClassName: edgion
  listeners:
    - name: https
      port: 443
      protocol: HTTPS
      tls:
        certificateRefs:
          - group: edgion.io
            kind: EdgionTls
            name: strict-mtls
```

### Example 2: Optional mTLS

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: optional-mtls
spec:
  secretRef:
    name: server-tls
  clientAuth:
    mode: Request  # Request but do not enforce
    caCertificateRefs:
      - name: trusted-client-ca
```

## Client Configuration

Test mTLS with curl:

```bash
curl --cert client.crt --key client.key \
     --cacert server-ca.crt \
     https://example.com/api
```

---

## Control Plane mTLS (conf_sync gRPC)

In addition to data plane mTLS (client ↔ Gateway), Edgion supports mTLS for the **control plane gRPC channel** between Controller and Gateway (`conf_sync`). This protects configuration synchronization traffic from eavesdropping and spoofing.

### Generate Certificates

Use `edgion-cli gen-certs` to generate a certificate bundle. Each bundle is identified by a name (`--cert-name`, default `primary`) and placed in a `{name}/` subdirectory:

```bash
edgion-cli gen-certs --output file --output-dir certs/grpc/ --cert-name primary
```

This creates `certs/grpc/primary/` containing:

| File | Purpose |
|------|---------|
| `ca.crt` | CA certificate (distributed to both sides) |
| `ca.key` | CA private key (signing only, do not deploy) |
| `server.crt` / `server.key` | Controller server certificate |
| `client.crt` / `client.key` | Gateway client certificate |

The server certificate includes `localhost`, `127.0.0.1`, `::1` as default SANs. Add custom SANs for production:

```bash
edgion-cli gen-certs --output file --output-dir certs/grpc/ --cert-name primary \
    --server-san edgion-controller.edgion-system.svc.cluster.local
```

To pre-stage a secondary certificate set for rotation:

```bash
edgion-cli gen-certs --output file --output-dir certs/grpc/ --cert-name secondary \
    --server-san edgion-controller.edgion-system.svc.cluster.local
```

### Controller Configuration

Add to `edgion-controller.toml`:

```toml
[conf_sync_security]
active = "primary"              # Selects the active certificate set

[[conf_sync_security.certs]]
name = "primary"
cert = "certs/grpc/primary/server.crt"
key  = "certs/grpc/primary/server.key"
ca   = "certs/grpc/primary/ca.crt"

# Optional: pre-stage a secondary cert set for rotation
[[conf_sync_security.certs]]
name = "secondary"
cert = "certs/grpc/secondary/server.crt"
key  = "certs/grpc/secondary/server.key"
ca   = "certs/grpc/secondary/ca.crt"
```

Certificate paths are resolved relative to `work_dir`.

### Gateway Configuration

Add to `edgion-gateway.toml`:

```toml
[conf_sync_security]
active = "primary"

[[conf_sync_security.certs]]
name = "primary"
cert = "certs/grpc/primary/client.crt"
key  = "certs/grpc/primary/client.key"
ca   = "certs/grpc/primary/ca.crt"

# Optional: pre-stage a secondary cert set for rotation
[[conf_sync_security.certs]]
name = "secondary"
cert = "certs/grpc/secondary/client.crt"
key  = "certs/grpc/secondary/client.key"
ca   = "certs/grpc/secondary/ca.crt"
```

When mTLS is enabled, the Gateway automatically upgrades the `server_addr` connection from `http://` to `https://`.

> **Backward compatibility**: The legacy single-cert format is still supported and requires no migration:
> ```toml
> [conf_sync_security.tls]
> cert = "certs/grpc/server.crt"
> key  = "certs/grpc/server.key"
> ca   = "certs/grpc/ca.crt"
> ```

### Configuration Reference

**`[conf_sync_security]`** — top-level section

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `active` | String | — | Name of the active cert set. Required when multiple `[[certs]]` entries exist. |

**`[[conf_sync_security.certs]]`** — certificate set (array of tables)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | String | — | Unique name for this cert set |
| `cert` | String | — | PEM certificate path (relative to work_dir) |
| `key` | String | — | PEM private key path |
| `ca` | String | — | CA certificate path (for verifying the peer) |
| `skip_tls` | Boolean | `false` | Emergency fallback: skip TLS, run plaintext (logs ERROR) |

### Certificate Selection

| Configuration | Active Certificate |
|--------------|-------------------|
| Single `[[certs]]` entry, no `active` | That entry (auto-selected) |
| Multiple `[[certs]]` entries + `active = "name"` | Entry matching `name` |
| Multiple `[[certs]]` entries, no `active` | Startup error |
| No `[[certs]]`, legacy `[tls]` present | Legacy `[tls]` entry |
| No mTLS config | Plaintext gRPC |

### Behavior Matrix

| Configuration | Result |
|--------------|--------|
| No `[conf_sync_security]` section | Plaintext gRPC (backward compatible) |
| Valid active cert | mTLS gRPC |
| `active` not matching any cert name | Startup failure |
| Missing or invalid cert files | Startup failure (Controller) / connection failure with retry (Gateway) |
| `skip_tls = true` | Plaintext with ERROR-level warning logs |

### Certificate Rotation

To rotate certificates without losing rollback capability:

1. Generate a new cert set: `edgion-cli gen-certs --cert-name secondary ...`
2. Add the `secondary` entry to both Controller and Gateway TOML, keeping `active = "primary"`
3. Restart both processes to validate the new cert files load correctly
4. Switch: set `active = "secondary"` in both configs and restart
5. If issues arise, revert `active = "primary"` and restart to roll back instantly

### Kubernetes Deployment

For K8s deployments, generate certificates as K8s Secrets:

```bash
edgion-cli gen-certs --output k8s --cert-name primary
```

Mount the secrets into Controller and Gateway pods, and reference the mount paths in the TOML configuration.

### Verification

After enabling mTLS, verify by checking the logs:

```
# Controller log
conf_sync mTLS enabled for gRPC server

# Gateway log
conf_sync mTLS enabled for gRPC client
```

You can also confirm by attempting a plaintext gRPC connection to the Controller's gRPC port — it should be rejected.

## Related Documentation

- [TLS Termination](../gateway/tls/tls-termination.md)
- [Backend TLS](../../user-guide/http-route/backends/backend-tls.md)
- [edgion-cli gen-certs](../edgion-cli.md)
