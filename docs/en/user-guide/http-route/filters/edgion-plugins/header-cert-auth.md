# Header Cert Auth Plugin

> **🔌 Edgion Extension**
> 
> HeaderCertAuth is a client certificate authentication plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

## Overview

Header Cert Auth authenticates clients by verifying their certificates. Two modes are supported:
- **Header mode**: Reads client certificate from an HTTP Header (for scenarios where a front proxy handles TLS termination)
- **mTLS mode**: Reads certificate from the mTLS connection context (direct TLS connection)

## Quick Start

### Create CA Certificate Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: client-ca-cert
type: Opaque
data:
  ca.crt: <base64-encoded-ca-certificate>
```

### Configure the Plugin

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cert-auth-plugin
spec:
  requestPlugins:
    - enable: true
      type: HeaderCertAuth
      config:
        mode: Header
        certificateHeaderName: X-Client-Cert
        caSecretRefs:
          - name: client-ca-cert
```

---

## Configuration Reference

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `mode` | String | No | `Header` | Certificate source mode: `Header` / `Mtls` |
| `certificateHeaderName` | String | No | `"X-Client-Cert"` | Header name containing the certificate (Header mode) |
| `certificateHeaderFormat` | String | No | `Base64Encoded` | Certificate encoding: `Base64Encoded` / `UrlEncoded` |
| `hideCredentials` | Boolean | No | `true` | Remove certificate header from request |
| `caSecretRefs` | Array | Yes* | `[]` | CA certificate Secret references |
| `verifyDepth` | Integer | No | `1` | Certificate chain verification depth |
| `skipConsumerLookup` | Boolean | No | `false` | Skip consumer identity lookup |
| `consumerBy` | String | No | `SanOrCn` | Consumer identity extraction: `SanOrCn` / `San` / `Cn` |
| `allowAnonymous` | Boolean | No | `false` | Allow access without certificate |
| `errorStatus` | Integer | No | `401` | Status code on verification failure |
| `errorMessage` | String | No | `"TLS certificate failed verification"` | Error message on failure |
| `authFailureDelayMs` | Integer | No | `0` | Auth failure delay (ms) |

\* Required in Header mode.

---

## Usage Scenarios

### Scenario 1: Header Mode (Front Proxy + Certificate Forwarding)

```yaml
requestPlugins:
  - enable: true
    type: HeaderCertAuth
    config:
      mode: Header
      certificateHeaderName: X-Client-Cert
      certificateHeaderFormat: urlEncoded
      caSecretRefs:
        - name: client-ca-cert
      verifyDepth: 2
      hideCredentials: true
```

### Scenario 2: mTLS Mode

```yaml
requestPlugins:
  - enable: true
    type: HeaderCertAuth
    config:
      mode: Mtls
      consumerBy: sanOrCn
```

### Scenario 3: Allow Anonymous Access

```yaml
requestPlugins:
  - enable: true
    type: HeaderCertAuth
    config:
      mode: Header
      caSecretRefs:
        - name: client-ca-cert
      allowAnonymous: true
```

---

## Behavior Details

- **Header mode**: Reads PEM certificate from specified header, validates against CA certificates in `caSecretRefs`
- **mTLS mode**: Obtains the verified client certificate directly from the TLS handshake context
- **Consumer identity**: Extracts identity from certificate SAN or CN based on `consumerBy` configuration, sets `X-Consumer-Username`
- With `hideCredentials: true`, the certificate header is not forwarded upstream

---

## Troubleshooting

### Problem 1: Certificate Verification Fails

**Cause**: CA certificate mismatch or incomplete certificate chain.

**Solution**:
```bash
openssl verify -CAfile ca.crt client.crt
```

### Problem 2: Certificate Parsing Fails from Header

**Cause**: Certificate encoding format doesn't match `certificateHeaderFormat`.

**Solution**: Ensure the front proxy uses the same encoding format as configured.

---

## Related Docs

- [mTLS Configuration](../../../../ops-guide/infrastructure/mtls.md)
- [TLS Termination](../../../../ops-guide/gateway/tls/tls-termination.md)
- [Basic Auth](./basic-auth.md)
