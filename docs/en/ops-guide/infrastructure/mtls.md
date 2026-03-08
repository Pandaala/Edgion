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
| mode | string | | None/Request/Require |
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

## Related Documentation

- [TLS Termination](../gateway/tls/tls-termination.md)
- [Backend TLS](../../user-guide/http-route/backends/backend-tls.md)
