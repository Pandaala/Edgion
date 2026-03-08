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
| validation.hostname | string | ✓ | Hostname for validation |

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
