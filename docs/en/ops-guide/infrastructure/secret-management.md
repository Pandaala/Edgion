# Secret Management

This document describes how Edgion uses Kubernetes Secrets for TLS, authentication plugins, and backend connections, along with operational recommendations.

## Common Uses of Secrets

- Gateway TLS certificates: `listeners[].tls.certificateRefs`
- Plugin keys: e.g., `jwt-auth`, `openid-connect`, `basic-auth`
- Backend authentication materials: e.g., mTLS client certificates

## Management Recommendations

1. Split Secrets by purpose; do not put unrelated credentials in a single object.
2. Use least-privilege RBAC to restrict Secret visibility.
3. Enable Secret rotation policies in production environments.
4. After changes, observe resource status and gateway logs to confirm they have taken effect.

## Troubleshooting

### Symptom: Gateway/Route Status Abnormal

- Check if the Secret name and namespace are correct.
- Check if a `ReferenceGrant` allows cross-namespace references.
- Check if the Secret key names match the plugin field names.

### Symptom: TLS Handshake Failure

- Check if the certificate chain and private key match.
- Check if the certificate has expired.
- Check if SNI matches the certificate SAN.

## Related Documentation

- [TLS Termination](../gateway/tls/tls-termination.md)
- [EdgionTls Extension](../gateway/tls/edgion-tls.md)
- [Cross-Namespace Reference](./reference-grant.md)
