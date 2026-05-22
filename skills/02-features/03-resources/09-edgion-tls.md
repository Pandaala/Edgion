---
name: edgion-tls-features
description: Full EdgionTls CRD schema: mTLS mutual authentication, TLS version control, cipher suites, OCSP, session management.
---

# EdgionTls Resource

> API: `edgion.io/v1` | Scope: Namespaced

EdgionTls is Edgion's extended TLS configuration resource, providing advanced TLS features beyond the Gateway built-in `tls.certificateRefs`.

## Full Schema

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: my-tls
  namespace: default
  annotations:
    edgion.io/expose-client-cert: "true"          # Expose client certificate information to request headers
spec:
  parentRefs:                                      # Attach to Gateway Listener
    - name: my-gateway
      sectionName: https

  hosts:                                           # Required: list of applicable hostnames
    - "api.example.com"
    - "*.example.com"

  secretRef:                                       # Required: TLS certificate Secret reference
    name: my-cert-secret
    namespace: cert-ns                             # Cross-namespace requires ReferenceGrant

  clientAuth:                                      # mTLS client authentication configuration
    mode: Mutual                                   # Terminate | Mutual | OptionalMutual
    caSecretRef:                                   # Required when mode=Mutual/OptionalMutual
      name: ca-cert-secret
      namespace: cert-ns
    verifyDepth: 1                                 # Certificate chain verification depth (1-9, default 1)
    allowedSans:                                   # SAN allowlist
      - "client.example.com"
    allowedCns:                                    # CN allowlist
      - "My Client"

  minTlsVersion: Tls12                             # Minimum TLS version

  ciphers:                                         # OpenSSL-format cipher suite list
    - "ECDHE-ECDSA-AES256-GCM-SHA384"
    - "ECDHE-RSA-AES256-GCM-SHA384"

  extend:                                          # Extended/experimental TLS configuration
    preferServerCiphers: true                      # Prefer the server-side cipher suite order

    ocspStapling:                                  # OCSP Stapling
      enabled: true
      refreshIntervalSeconds: 3600
      failOpen: false

    sessionTicket:                                 # Session Ticket
      enabled: true
      lifetimeSeconds: 7200
      rotationIntervalSeconds: 3600

    sessionCache:                                  # Session Cache
      enabled: true
      maxEntries: 10000
      ttlSeconds: 3600

    revocationCheck:                               # Certificate revocation check
      mode: ocsp                                   # off | ocsp | crl
      failOpen: false

    earlyData:                                     # TLS 1.3 0-RTT
      enabled: false
      rejectOnReplay: true
```

## spec Field Details

### Basic Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `parentRefs` | `Vec<ParentReference>` | No | Attach to Gateway Listener |
| `hosts` | `Vec<String>` | Yes | List of applicable hostnames |
| `secretRef` | `SecretObjectReference` | Yes | TLS certificate Secret reference |
| `minTlsVersion` | `TlsVersion?` | No | Minimum TLS version |
| `ciphers` | `Vec<String>?` | No | OpenSSL-format cipher suites |

### TlsVersion Enum

| Value | Description |
|-------|-------------|
| `Tls10` | TLS 1.0 (not recommended) |
| `Tls11` | TLS 1.1 (not recommended) |
| `Tls12` | TLS 1.2 (recommended minimum version) |
| `Tls13` | TLS 1.3 |

### clientAuth — mTLS Configuration

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | `ClientAuthMode` | `Terminate` | Authentication mode |
| `caSecretRef` | `SecretObjectReference?` | — | CA certificate Secret (required for Mutual/OptionalMutual) |
| `verifyDepth` | `u8` | `1` | Certificate chain verification depth (1-9) |
| `allowedSans` | `Vec<String>?` | — | Subject Alternative Name allowlist |
| `allowedCns` | `Vec<String>?` | — | Common Name allowlist |

**ClientAuthMode enum**:

| Value | Description |
|-------|-------------|
| `Terminate` | One-way TLS: only verifies the server certificate (default) |
| `Mutual` | Mutual TLS: a valid client certificate must be provided |
| `OptionalMutual` | Optional mutual TLS: the client certificate is optional but verified when provided |

### extend — Extended TLS Configuration

#### ocspStapling

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | `bool` | `false` | Enable OCSP Stapling |
| `refreshIntervalSeconds` | `u64?` | — | OCSP response refresh interval |
| `failOpen` | `bool` | `false` | Whether to allow when OCSP is unavailable |

#### sessionTicket

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | `bool` | `false` | Enable Session Ticket |
| `lifetimeSeconds` | `u64?` | — | Ticket lifetime |
| `rotationIntervalSeconds` | `u64?` | — | Key rotation interval |

#### sessionCache

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | `bool` | `false` | Enable Session Cache |
| `maxEntries` | `u64?` | — | Maximum cache entries |
| `ttlSeconds` | `u64?` | — | Entry TTL |

#### revocationCheck

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | `RevocationMode` | `Off` | `Off` / `Ocsp` / `Crl` |
| `failOpen` | `bool` | `false` | Allow when revocation data is unavailable |

#### earlyData (TLS 1.3 0-RTT)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | `bool` | `false` | Enable 0-RTT |
| `rejectOnReplay` | `bool` | `false` | Reject when replay is detected |

## Relationship to Gateway TLS

| Feature | Gateway `tls` | EdgionTls |
|---------|---------------|-----------|
| Basic certificate binding | ✅ certificateRefs | ✅ secretRef |
| mTLS client authentication | ❌ | ✅ clientAuth |
| TLS version control | ❌ | ✅ minTlsVersion |
| Cipher suite control | ❌ | ✅ ciphers |
| OCSP Stapling | ❌ | ✅ extend.ocspStapling |
| Session management | ❌ | ✅ extend.sessionTicket/sessionCache |

When using EdgionTls, set `tls.options.edgion.io/cert-provider: "edgion-tls"` on the Gateway Listener.
