# Secret Management

Edgion consumes Kubernetes Secrets for TLS certificates, plugin credentials, and the Controller admin password. This page covers how Secrets are referenced, the Secret types involved, the auto-generated admin credential, and operational recommendations.

## How Edgion uses Secrets

| Use | Where it is referenced | Typical Secret type |
|-----|------------------------|---------------------|
| Gateway TLS certificate | `listeners[].tls.certificateRefs` on a Gateway | `kubernetes.io/tls` |
| Extended TLS (mTLS, ciphers, OCSP) | `EdgionTls.spec.secretRef` and `clientAuth.caSecretRef` | `kubernetes.io/tls` (server), `Opaque` (CA) |
| Plugin credentials | Plugins such as `jwt-auth`, `basic-auth`, `openid-connect` | `Opaque` |
| Backend authentication | Backend mTLS client certificates | `kubernetes.io/tls` |
| Controller admin password | Auto-generated `edgion-admin-credentials` | `Opaque` |

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

## Admin credentials (auto-generated)

When the Controller starts **without** a configured `[local_auth]` section, it auto-generates a random admin password and stores it in a Secret named `edgion-admin-credentials` in the `edgion-system` namespace. This applies in both Kubernetes mode and local mode.

Retrieve the generated password:

```bash
# Kubernetes mode
kubectl -n edgion-system get secret edgion-admin-credentials \
  -o jsonpath='{.data.password}' | base64 -d

# Local mode (file-based config)
cat {conf_dir}/Secret_edgion-system_edgion-admin-credentials.yaml
```

> **Center is different**: the Center process does **not** auto-generate credentials. The operator must supply `[local_auth]` or `[auth]`; otherwise business routes fail closed with 503 while health endpoints stay available.

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

### Cannot log in to the admin API

- Retrieve the auto-generated password from `edgion-admin-credentials` (see above), or configure `[local_auth]` explicitly.

## FAQ

### Q1: Where is the Controller admin password stored?

**A**: In the `edgion-admin-credentials` Secret in the `edgion-system` namespace, auto-generated on first start when `[local_auth]` is not configured. In local mode it is written to `{conf_dir}/Secret_edgion-system_edgion-admin-credentials.yaml`.

### Q2: Can I reference a Secret in a different namespace?

**A**: Yes, but you must create a `ReferenceGrant` in the Secret's namespace authorizing the reference. See [Cross-Namespace Reference](./reference-grant.md).

### Q3: My plugin reports a missing credential even though the Secret exists. Why?

**A**: The key names inside the Secret must match the field names the plugin expects. Check the plugin's documentation for the required keys and confirm they are present (and base64-encoded) in the Secret's `data`.

## Related documents

- [TLS Termination](../gateway/tls/tls-termination.md)
- [EdgionTls](../gateway/tls/edgion-tls.md)
- [mTLS Configuration](./mtls.md)
- [Cross-Namespace Reference](./reference-grant.md)
