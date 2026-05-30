# TLS Termination

Terminate client TLS at the Gateway using the standard Gateway API listener configuration.

## Overview

**TLS termination** means the Gateway decrypts the client's TLS connection at the listener, inspects the plaintext request (host, path, headers), and then forwards it to the backend — either in plaintext or re-encrypted over a new TLS connection:

```
Client --[TLS]--> Gateway (decrypt) --[plaintext or re-encrypted TLS]--> Backend
```

You need TLS termination whenever the Gateway must route on Layer 7 (HTTPRoute / GRPCRoute) for an HTTPS endpoint, because routing decisions require the decrypted request.

This is the standard Gateway API approach: an HTTPS `Listener` with `tls.mode: Terminate` and a `certificateRefs` entry pointing at a `kubernetes.io/tls` Secret. It contrasts with **TLS passthrough** (`tls.mode: Passthrough` + `TLSRoute`), where the Gateway forwards the encrypted bytes to the backend without decrypting — see [TLS Passthrough Listener](../listeners/tls-passthrough.md).

If you need TLS features beyond the Gateway API (mTLS, minimum TLS version, custom cipher list), use the [EdgionTls extension](./edgion-tls.md) instead of — or alongside — the standard listener configuration.

## Configuration

### Method 1: Gateway listener (standard Gateway API)

A complete, applyable Gateway with one HTTPS listener that terminates TLS:

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
      hostname: "example.com"
      tls:
        mode: Terminate
        certificateRefs:
          - name: example-com-tls
```

Key fields:

| Field | Value | Notes |
|-------|-------|-------|
| `protocol` | `HTTPS` | Use `HTTPS` for L7 termination; `TLS` is for passthrough/termination of raw TLS streams |
| `tls.mode` | `Terminate` | Decrypt at the Gateway. `Passthrough` forwards encrypted bytes to the backend |
| `tls.certificateRefs[].name` | Secret name | Must reference a `kubernetes.io/tls` Secret in the same namespace as the Gateway, unless a `ReferenceGrant` allows cross-namespace access |

### Method 2: EdgionTls extension

> **🔌 Edgion extension**
>
> `EdgionTls` is an Edgion custom CRD that adds TLS capabilities the standard Gateway API does not cover — mutual TLS, minimum TLS version, and OpenSSL-format cipher lists.

When you need those extra capabilities, attach an EdgionTls resource to the listener instead of (or in addition to) `certificateRefs`. The schema and all parameters are documented in [EdgionTls extension](./edgion-tls.md):

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: example-tls
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: https
  hosts:
    - example.com
  secretRef:
    name: example-com-tls
    namespace: production
  # Minimum accepted TLS version (optional).
  minTlsVersion: TLS1_2
```

## Certificate management

### Creating a TLS Secret

The certificate Secret must be of type `kubernetes.io/tls` and carry the `tls.crt` and `tls.key` keys.

```bash
# Create directly from PEM files (recommended)
kubectl create secret tls example-com-tls \
  --cert=path/to/tls.crt \
  --key=path/to/tls.key \
  -n production
```

Or declare it as YAML. The data fields hold base64-encoded PEM:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: example-com-tls
  namespace: production
type: kubernetes.io/tls
data:
  tls.crt: <base64-encoded-cert>
  tls.key: <base64-encoded-key>
```

⚠️ **Never commit real private keys to Git or paste them into documentation.** The `<base64-encoded-cert>` / `<base64-encoded-key>` values above are placeholders.

### Certificate chain

If clients need the full chain to validate, concatenate the leaf certificate followed by the intermediate(s) into `tls.crt` (leaf first):

```text
-----BEGIN CERTIFICATE-----
(leaf / server certificate)
-----END CERTIFICATE-----
-----BEGIN CERTIFICATE-----
(intermediate CA certificate)
-----END CERTIFICATE-----
```

## Examples

### Example 1: Single-domain certificate

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
      hostname: "example.com"
      tls:
        mode: Terminate
        certificateRefs:
          - name: example-com-tls
```

### Example 2: Wildcard certificate

A wildcard certificate covers exactly one subdomain label (RFC 6125 §6.4.3):

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: main-gateway
  namespace: production
spec:
  gatewayClassName: edgion
  listeners:
    - name: https-wildcard
      port: 443
      protocol: HTTPS
      hostname: "*.example.com"
      tls:
        mode: Terminate
        certificateRefs:
          - name: wildcard-example-com-tls
```

> **Wildcard depth note**: `*.example.com` covers `api.example.com` and `app.example.com`, but **not** the apex `example.com` and **not** `deep.sub.example.com`. Edgion selects the certificate by SNI using the RFC 6125 single-label rule, so a connection to a host outside that one-label range fails the TLS handshake unless a more specific listener / certificate (for example `*.sub.example.com`, or an explicit `example.com` listener) is also configured. Note that HTTPRoute / TLSRoute hostname *route* matching follows Gateway API §4.3 and is multi-label — only certificate selection is RFC 6125-strict.

### Example 3: Multiple certificates / SNI on one Gateway

To serve several domains with different certificates, declare one listener per hostname. The Gateway picks the matching certificate from the client's SNI:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: main-gateway
  namespace: production
spec:
  gatewayClassName: edgion
  listeners:
    # Apex + www served by one certificate
    - name: https-apex
      port: 443
      protocol: HTTPS
      hostname: "example.com"
      tls:
        mode: Terminate
        certificateRefs:
          - name: example-com-tls
    # All single-label subdomains served by a wildcard certificate
    - name: https-wildcard
      port: 443
      protocol: HTTPS
      hostname: "*.example.com"
      tls:
        mode: Terminate
        certificateRefs:
          - name: wildcard-example-com-tls
    # A second, unrelated domain on the same Gateway
    - name: https-other
      port: 443
      protocol: HTTPS
      hostname: "api.partner.com"
      tls:
        mode: Terminate
        certificateRefs:
          - name: partner-com-tls
```

Each listener owns its own certificate; the Gateway selects per connection based on the SNI sent by the client.

## Related documents

- [TLS Passthrough Listener](../listeners/tls-passthrough.md)
- [HTTPS Listener](../listeners/https.md)
- [EdgionTls extension](./edgion-tls.md)
- [ACME automatic certificates](./acme.md)
