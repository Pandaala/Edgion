# Secret Management

Edgion consumes Kubernetes Secrets for TLS certificates, plugin credentials, and the Controller admin token. This page covers how Secrets are referenced, the Secret types involved, the admin credential Secret, and operational recommendations.

## How Edgion uses Secrets

| Use | Where it is referenced | Typical Secret type |
|-----|------------------------|---------------------|
| Gateway TLS certificate | `listeners[].tls.certificateRefs` on a Gateway | `kubernetes.io/tls` |
| Extended TLS (mTLS, ciphers, OCSP) | `EdgionTls.spec.secretRef` and `clientAuth.caSecretRef` | `kubernetes.io/tls` (server), `Opaque` (CA) |
| Plugin credentials | Plugins such as `jwt-auth`, `basic-auth`, `openid-connect` | `Opaque` |
| Backend authentication | Backend mTLS client certificates | `kubernetes.io/tls` |
| Controller admin token | Operator-created `edgion-admin-credentials` (single `token` key, injected as the `EDGION_ADMIN_TOKEN` env var) | `Opaque` |

### TLS certificate Secret

Standard Kubernetes TLS Secrets carry `tls.crt` and `tls.key`:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: my-tls-secret
  namespace: production
type: kubernetes.io/tls
data:
  tls.crt: <base64-encoded-cert>
  tls.key: <base64-encoded-key>
```

A CA Secret used for mTLS client verification carries the CA certificate, typically as an `Opaque` Secret with a `ca.crt` key:

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

⚠️ Never commit real certificate or key material to source control. All values above are placeholders.

## Admin token (`edgion-admin-credentials`)

The Controller Admin API uses **token-only** authentication. It reads a single pre-shared
admin token from the `edgion-admin-credentials` Secret (namespace: `edgion-system`), which
holds one key, `token`. This Secret must be created by the operator — the Controller no
longer auto-generates it.

The recommended Kubernetes approach is to use `deploy.sh`, which creates the Secret
idempotently and wires it into the controller Deployment as the `EDGION_ADMIN_TOKEN` env var
(via `valueFrom.secretKeyRef` on the `token` key):

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: edgion-admin-credentials
  namespace: edgion-system
type: Opaque
data:
  token: <base64-encoded-admin-token>
```

```bash
# Retrieve the admin token after running deploy.sh
kubectl -n edgion-system get secret edgion-admin-credentials \
  -o jsonpath='{.data.token}' | base64 -d
```

Alternatively, set the token directly via the top-level `admin_token` field in the Controller
config (prefer env injection from a Secret over plaintext config).

When no admin token is configured (and no `cli_tokens` are set), the server still starts but
all Admin API business routes return **503 fail-close**. `/health`, `/ready`, `/metrics`, and
`/api/v1/auth/status` remain accessible.

See `skills/02-features/02-config/03-auth-bootstrap.md` for the full bootstrap matrix.

## Management recommendations

1. Split Secrets by purpose; do not put unrelated credentials in a single object.
2. Use least-privilege RBAC to restrict which workloads can read each Secret.
3. Enable a Secret rotation policy in production.
4. After changing a Secret, watch the referencing resource's status and the gateway logs to confirm the change took effect.

## Troubleshooting

### Gateway or Route status is abnormal after referencing a Secret

- Confirm the Secret name and namespace are correct.
- Confirm a [ReferenceGrant](./reference-grant.md) authorizes the reference when the Secret is in another namespace.
- Confirm the Secret key names match what the consumer expects (`tls.crt`/`tls.key` for TLS, `ca.crt` for a CA, plugin-specific keys for plugins).

### TLS handshake failure

- Confirm the certificate chain and the private key match.
- Confirm the certificate has not expired.
- Confirm the SNI presented by the client matches a SAN in the certificate.

### Admin API returns 401/403/503

- Retrieve the token from `edgion-admin-credentials` (see above — created by `deploy.sh`), or configure `admin_token` explicitly. A 503 means no admin token and no `cli_tokens` are configured; a 401 means the supplied token is unknown; a 403 means the token's RBAC policy denies the operation.

## FAQ

### Q1: Where is the Controller admin token stored?

**A**: In the `edgion-admin-credentials` Secret (single `token` key) in the `edgion-system` namespace, created by the operator (e.g., via `deploy.sh`). The controller Deployment injects the `token` value as the `EDGION_ADMIN_TOKEN` env var. The Controller no longer auto-generates this Secret.

### Q2: Can I reference a Secret in a different namespace?

**A**: Yes, but you must create a `ReferenceGrant` in the Secret's namespace authorizing the reference. See [Cross-Namespace Reference](./reference-grant.md).

### Q3: My plugin reports a missing credential even though the Secret exists. Why?

**A**: The key names inside the Secret must match the field names the plugin expects. Check the plugin's documentation for the required keys and confirm they are present (and base64-encoded) in the Secret's `data`.

## Related documents

- [TLS Termination](../gateway/tls/tls-termination.md)
- [EdgionTls](../gateway/tls/edgion-tls.md)
- [mTLS Configuration](./mtls.md)
- [Cross-Namespace Reference](./reference-grant.md)
