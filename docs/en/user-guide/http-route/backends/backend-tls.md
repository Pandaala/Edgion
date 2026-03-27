# Backend TLS

Configure TLS connections from the Gateway to backend services.

## Overview

Use BackendTLSPolicy to configure backend TLS:

```
Client → [TLS] → Gateway → [TLS] → Backend
```

## Configuring BackendTLSPolicy

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: BackendTLSPolicy
metadata:
  name: backend-tls
spec:
  targetRefs:
    - group: ""
      kind: Service
      name: secure-backend
  validation:
    caCertificateRefs:
      - group: ""
        kind: Secret
        name: backend-ca
    hostname: backend.internal
```

## Configuration Reference

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| targetRefs | array | ✓ | Target Service list |
| validation.caCertificateRefs | array | ✓ | CA certificate Secret |
| validation.hostname | string | ✓ | Hostname for SNI and certificate validation |
| validation.subjectAltNames | array | | Additional SAN validation rules |
| validation.subjectAltNames[].type | string | ✓ | `Hostname` or `URI` |
| validation.subjectAltNames[].hostname | string | | DNS name (when type=Hostname), supports wildcards (`*.example.com`) |
| validation.subjectAltNames[].uri | string | | Full URI (when type=URI), e.g. SPIFFE ID |

## CA Certificate Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: backend-ca
type: kubernetes.io/tls
data:
  ca.crt: <base64-encoded-ca-cert>
```

## Subject Alternative Name Validation

When `subjectAltNames` is configured, the Gateway validates the backend certificate's SAN during TLS handshake. The connection is rejected if no SAN matches.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: BackendTLSPolicy
metadata:
  name: backend-san-check
spec:
  targetRefs:
    - kind: Service
      name: secure-backend
  validation:
    caCertificateRefs:
      - kind: Secret
        name: backend-ca
    hostname: backend.internal
    subjectAltNames:
      - type: Hostname
        hostname: backend.internal
      - type: URI
        uri: spiffe://cluster.local/ns/default/sa/backend
```

Matching rules:
- **Hostname**: exact match or wildcard (`*.example.com` matches single-level subdomains)
- **URI**: exact match (e.g. SPIFFE IDs)
- Any single match across all entries is sufficient
- Omitting `subjectAltNames` preserves default behavior (hostname-only validation)

## Examples

### Example 1: Internal Service mTLS

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: BackendTLSPolicy
metadata:
  name: internal-mtls
spec:
  targetRefs:
    - kind: Service
      name: internal-api
  validation:
    caCertificateRefs:
      - kind: Secret
        name: internal-ca
    hostname: internal-api.svc.cluster.local
```

## Related Documentation

- [Service Reference](./service-ref.md)
- [mTLS Configuration](../../../ops-guide/infrastructure/mtls.md)
