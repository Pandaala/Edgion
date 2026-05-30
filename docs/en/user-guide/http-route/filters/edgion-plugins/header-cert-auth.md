# Header Cert Auth Plugin

> **🔌 Edgion extension**
> 
> HeaderCertAuth is a client certificate authentication plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

## Overview

Header Cert Auth authenticates clients by verifying their certificates. Two source modes are supported:
- **Header mode** (`mode: Header`): Reads the client certificate from an HTTP header (for scenarios where a front proxy / load balancer terminates TLS and forwards the certificate)
- **Connection mode** (`mode: Connection`): Reuses the verified client certificate from the mTLS connection context (direct TLS connection)

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
| `mode` | Enum | No | `Header` | Certificate source mode. Allowed values: `Header` / `Connection` |
| `certificateHeaderName` | String | No | `"X-Client-Cert"` | Header name containing the certificate (Header mode only) |
| `certificateHeaderFormat` | Enum | No | `Base64Encoded` | Certificate encoding (Header mode). Allowed values: `Base64Encoded` (base64 PEM body without delimiters) / `UrlEncoded` (URL-encoded full PEM) |
| `hideCredentials` | Boolean | No | `true` | Remove the source certificate header before forwarding upstream |
| `caSecretRefs` | Array | Yes* | `[]` | CA Secret references used to verify the client certificate. All certs in the referenced Secrets are loaded as trust anchors; include intermediates if the leaf is signed by one |
| `skipConsumerLookup` | Boolean | No | `false` | Skip consumer mapping and pass the cert DN/SAN through directly |
| `consumerBy` | Enum | No | `SanOrCn` | Consumer identity resolution. Allowed values: `SanOrCn` (first SAN, fall back to CN) / `Cn` (CN only) / `Fingerprint` (cert SHA-256 fingerprint) |
| `upstreamHeaders` | Object | No | see sub-table | Upstream header names for identity propagation |
| `allowAnonymous` | Boolean | No | `false` | Allow the request as anonymous when authentication fails |
| `errorStatus` | Integer | No | `401` | Status code on verification failure (must be 400..=599) |
| `errorMessage` | String | No | `"TLS certificate failed verification"` | Error message on failure |
| `authFailureDelayMs` | Integer | No | `0` | Fixed delay (ms) before returning an auth-failure response |

\* Required in Header mode (Connection mode obtains the verified cert from the TLS handshake and does not require `caSecretRefs`).

### `upstreamHeaders` sub-fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `consumerHeader` | String | `"X-Consumer-Username"` | Header for the resolved consumer identity |
| `dnHeader` | String | `"X-Client-Cert-Dn"` | Header for the certificate subject DN |
| `sanHeader` | String | `"X-Client-Cert-San"` | Header for the first SAN value |
| `fingerprintHeader` | String | `"X-Client-Cert-Fingerprint"` | Header for the certificate SHA-256 fingerprint |

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
      certificateHeaderFormat: UrlEncoded
      caSecretRefs:
        - name: client-ca-cert
      hideCredentials: true
```

### Scenario 2: Connection (mTLS) Mode

Reuse the certificate already verified during the TLS handshake. No `caSecretRefs` needed.

```yaml
requestPlugins:
  - enable: true
    type: HeaderCertAuth
    config:
      mode: Connection
      consumerBy: SanOrCn
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

- **Header mode**: Reads the certificate from the configured header, validates against the CA certificates in `caSecretRefs`
- **Connection mode**: Obtains the verified client certificate directly from the mTLS handshake context
- **Consumer identity**: Extracts identity from the certificate SAN, CN, or fingerprint based on `consumerBy`, and writes it to `upstreamHeaders.consumerHeader` (default `X-Consumer-Username`)
- With `hideCredentials: true` (the default), the source certificate header is not forwarded upstream

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

## Security Best Practices

### ✅ Recommended

- Supply a trusted `caSecretRefs` bundle and keep verification on; include intermediate CAs when the leaf is signed by one.
- In Header mode, ensure only the trusted TLS terminator can set `certificateHeaderName`; strip that header from untrusted ingress so clients cannot spoof a certificate.
- Restrict access with `consumerBy` plus upstream authorization where the set of clients is known.
- Keep `hideCredentials: true` (the default) unless the backend itself needs the certificate.
- Prefer Connection (mTLS) mode when the gateway terminates TLS directly — the certificate is verified during the handshake and cannot be header-spoofed.

### ❌ To avoid

- Trusting `certificateHeaderName` from sources other than your TLS terminator (header spoofing).
- Enabling `allowAnonymous: true` on routes that must require a client certificate.
- Forwarding the certificate header upstream (`hideCredentials: false`) when the backend does not need it.

---

## Related Docs

- [mTLS Configuration](../../../../ops-guide/infrastructure/mtls.md)
- [TLS Termination](../../../../ops-guide/gateway/tls/tls-termination.md)
- [Basic Auth](./basic-auth.md)
- [JWT Auth](./jwt-auth.md)
