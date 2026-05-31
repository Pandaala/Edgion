# mTLS Configuration

> **🔌 Edgion extension**
> 
> Downstream (client→Gateway) mTLS is configured through the `EdgionTls` CRD; it is not part of standard Gateway API.

Configure Mutual TLS (mTLS) so that the Gateway verifies the client certificate in addition to presenting its own server certificate. Edgion supports mTLS in two independent places: the **data plane** (client → Gateway) via `EdgionTls`, and the **control plane** (Controller ↔ Gateway gRPC) via `conf_sync_security`.

## Overview

In one-way TLS the client verifies the server. mTLS adds the reverse check: the server also verifies the client's certificate against a trusted CA.

```
one-way TLS:   client ──verify server cert──► Gateway
mTLS:          client ──verify server cert──► Gateway
               client ◄──verify client cert── Gateway
```

For the full `EdgionTls` schema (TLS version control, cipher suites, OCSP, session management) see [EdgionTls](../gateway/tls/edgion-tls.md). This page focuses on the mTLS-relevant fields.

## Configuration

Enable mTLS by adding a `clientAuth` block to an `EdgionTls` resource and attaching it to an HTTPS Listener.

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: mtls-config
  namespace: production
spec:
  hosts:
    - "api.example.com"
  secretRef:
    name: my-tls-secret              # Server certificate
  clientAuth:
    mode: Mutual                     # Require a valid client certificate
    caSecretRef:
      name: client-ca                # CA that signs trusted client certs
```

### clientAuth reference

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `mode` | Enum | `Terminate` | Client authentication mode (see below) |
| `caSecretRef` | Object | none | CA certificate Secret. **Required** when `mode` is `Mutual` or `OptionalMutual` |
| `verifyDepth` | Integer | `1` | Certificate-chain verification depth (1-9) |
| `allowedSans` | Array\<String\> | none | Subject Alternative Name allowlist |
| `allowedCns` | Array\<String\> | none | Common Name allowlist |

#### `clientAuth.mode` values

| Mode | Description |
|------|-------------|
| `Terminate` | One-way TLS: only the server certificate is verified (default) |
| `Mutual` | Mutual TLS: the client **must** present a valid certificate |
| `OptionalMutual` | Optional mutual TLS: the client certificate is optional, but verified when provided |

### Attaching EdgionTls to a Listener

Reference the `EdgionTls` resource from the HTTPS Listener's `certificateRefs`, and set the `cert-provider` option so the Gateway uses the extended TLS stack:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: main-gateway
  namespace: production
spec:
  gatewayClassName: edgion
  listeners:
    - name: https
      port: 443
      protocol: HTTPS
      tls:
        options:
          edgion.io/cert-provider: "edgion-tls"
        certificateRefs:
          - group: edgion.io
            kind: EdgionTls
            name: mtls-config
```

## CA Certificate Secret

The CA Secret holds the certificate that signs trusted client certificates:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: client-ca
  namespace: production
type: Opaque
data:
  ca.crt: <base64-encoded-ca-cert>
```

⚠️ Never commit real certificate or key material to source control. The values above are placeholders.

## Common scenarios

### 1. Strict mTLS (client certificate required)

Every client must present a certificate signed by the trusted CA, or the handshake is rejected:

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: strict-mtls
  namespace: production
spec:
  hosts:
    - "api.example.com"
  secretRef:
    name: my-tls-secret
  clientAuth:
    mode: Mutual
    caSecretRef:
      name: client-ca
---
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: main-gateway
  namespace: production
spec:
  gatewayClassName: edgion
  listeners:
    - name: https
      port: 443
      protocol: HTTPS
      tls:
        options:
          edgion.io/cert-provider: "edgion-tls"
        certificateRefs:
          - group: edgion.io
            kind: EdgionTls
            name: strict-mtls
```

### 2. Optional mTLS (verify when present)

Allow both authenticated and anonymous clients, but verify any certificate that is offered — useful during a phased rollout:

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: optional-mtls
  namespace: production
spec:
  hosts:
    - "api.example.com"
  secretRef:
    name: my-tls-secret
  clientAuth:
    mode: OptionalMutual
    caSecretRef:
      name: client-ca
```

### 3. Restricting accepted client identities

Tighten `Mutual` mode further by allowing only specific client SANs or CNs:

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: restricted-mtls
  namespace: production
spec:
  hosts:
    - "api.example.com"
  secretRef:
    name: my-tls-secret
  clientAuth:
    mode: Mutual
    caSecretRef:
      name: client-ca
    verifyDepth: 2
    allowedSans:
      - "client.example.com"
    allowedCns:
      - "Trusted Client"
```

## Testing with curl

```bash
curl --cert client.crt --key client.key \
     --cacert server-ca.crt \
     https://api.example.com/api
```

A client without a valid certificate is rejected at the TLS handshake when `mode: Mutual`.

## FAQ

### Q1: The handshake fails with "certificate required". What is wrong?

**A**: The Listener is in `Mutual` mode and the client did not present a certificate, or presented one not signed by the CA in `caSecretRef`. Verify the client cert chains to that CA, and that `caSecretRef` points at the correct Secret.

### Q2: My `EdgionTls` resource is ignored — the Gateway uses a plain certificate. Why?

**A**: The Listener must opt in with `tls.options."edgion.io/cert-provider": "edgion-tls"`. Without that option the Listener falls back to the built-in `certificateRefs` handling and the `EdgionTls` extension fields (including `clientAuth`) are not applied.

### Q3: How do I expose the verified client identity to the backend?

**A**: Set the `edgion.io/expose-client-cert: "true"` annotation on the `EdgionTls` resource. The Gateway then forwards client certificate information to the backend via request headers.

### Q4: The certificate Secret is in another namespace. What do I need?

**A**: Create a `ReferenceGrant` in the Secret's namespace authorizing the reference. See [Cross-Namespace Reference](./reference-grant.md).

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

Add to `edgion-controller.yaml`:

```yaml
conf_sync_security:
  active: primary              # Selects the active certificate set
  certs:
    - name: primary
      cert: certs/grpc/primary/server.crt
      key: certs/grpc/primary/server.key
      ca: certs/grpc/primary/ca.crt
    # Optional: pre-stage a secondary cert set for rotation
    - name: secondary
      cert: certs/grpc/secondary/server.crt
      key: certs/grpc/secondary/server.key
      ca: certs/grpc/secondary/ca.crt
```

Certificate paths are resolved relative to `work_dir`.

### Gateway Configuration

Add to `edgion-gateway.yaml`:

```yaml
conf_sync_security:
  active: primary
  certs:
    - name: primary
      cert: certs/grpc/primary/client.crt
      key: certs/grpc/primary/client.key
      ca: certs/grpc/primary/ca.crt
    # Optional: pre-stage a secondary cert set for rotation
    - name: secondary
      cert: certs/grpc/secondary/client.crt
      key: certs/grpc/secondary/client.key
      ca: certs/grpc/secondary/ca.crt
```

When mTLS is enabled, the Gateway automatically upgrades the `server_addr` connection from `http://` to `https://`.

> **Backward compatibility**: The legacy single-cert format is still supported and requires no migration:
> ```yaml
> conf_sync_security:
>   tls:
>     cert: certs/grpc/server.crt
>     key: certs/grpc/server.key
>     ca: certs/grpc/ca.crt
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
2. Add the `secondary` entry to both Controller and Gateway YAML, keeping `active: primary`
3. Restart both processes to validate the new cert files load correctly
4. Switch: set `active: secondary` in both configs and restart
5. If issues arise, revert `active: primary` and restart to roll back instantly

### Kubernetes Deployment

For K8s deployments, generate certificates as K8s Secrets:

```bash
edgion-cli gen-certs --output k8s --cert-name primary
```

Mount the secrets into Controller and Gateway pods, and reference the mount paths in the YAML configuration.

### Verification

After enabling mTLS, verify by checking the logs:

```
# Controller log
conf_sync mTLS enabled for gRPC server

# Gateway log
conf_sync mTLS enabled for gRPC client
```

You can also confirm by attempting a plaintext gRPC connection to the Controller's gRPC port — it should be rejected.

## Related documents

- [EdgionTls](../gateway/tls/edgion-tls.md)
- [TLS Termination](../gateway/tls/tls-termination.md)
- [Backend TLS](../../user-guide/http-route/backends/backend-tls.md)
- [edgion-cli gen-certs](../edgion-cli.md)
