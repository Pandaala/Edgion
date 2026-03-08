# TLS Termination

Configure TLS termination for the Gateway.

## Overview

TLS termination means the Gateway decrypts the client's TLS connection, then forwards to the backend in plaintext or re-encrypted form:

```
Client -> [TLS] -> Gateway -> [Plaintext/TLS] -> Backend
```

## Configuration Methods

### Method 1: Gateway TLS Configuration

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: tls-gateway
spec:
  gatewayClassName: edgion
  listeners:
    - name: https
      port: 443
      protocol: HTTPS
      tls:
        mode: Terminate
        certificateRefs:
          - name: my-tls-secret
```

### Method 2: EdgionTls Extension

> **🔌 Edgion Extension**
> 
> `EdgionTls` is an Edgion custom CRD that provides richer TLS configuration options than the standard Gateway API.

Use the EdgionTls resource for additional TLS options:

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: advanced-tls
spec:
  secretRef:
    name: my-tls-secret
  minVersion: TLSv1.2
  cipherSuites:
    - TLS_AES_128_GCM_SHA256
    - TLS_AES_256_GCM_SHA384
```

## Certificate Management

### Creating a TLS Secret

```bash
# Create from files
kubectl create secret tls my-tls-secret \
  --cert=path/to/cert.pem \
  --key=path/to/key.pem

# Or use YAML
apiVersion: v1
kind: Secret
metadata:
  name: my-tls-secret
type: kubernetes.io/tls
data:
  tls.crt: <base64-encoded-cert>
  tls.key: <base64-encoded-key>
```

### Certificate Chain

If a full certificate chain is needed, append intermediate certificates to tls.crt:

```
-----BEGIN CERTIFICATE-----
(Server certificate)
-----END CERTIFICATE-----
-----BEGIN CERTIFICATE-----
(Intermediate certificate)
-----END CERTIFICATE-----
```

## Examples

### Example 1: Single Domain Certificate

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: single-domain
spec:
  gatewayClassName: edgion
  listeners:
    - name: https
      port: 443
      protocol: HTTPS
      hostname: "example.com"
      tls:
        certificateRefs:
          - name: example-com-tls
```

### Example 2: Wildcard Certificate

```yaml
listeners:
  - name: https
    port: 443
    protocol: HTTPS
    hostname: "*.example.com"
    tls:
      certificateRefs:
        - name: wildcard-example-com-tls
```

## Related Documentation

- [HTTPS Listener](../listeners/https.md)
- [EdgionTls Extension](./edgion-tls.md)
- [mTLS Configuration](../../infrastructure/mtls.md)
