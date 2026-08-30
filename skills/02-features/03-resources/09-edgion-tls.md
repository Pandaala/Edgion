---
name: edgion-tls-features
description: Full EdgionTls CRD schema: mTLS mutual authentication, TLS version control, cipher suites.
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
```

## spec Field Details

### Basic Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `parentRefs` | `Vec<ParentReference>` | No | Attach to Gateway Listener |
| `hosts` | `Vec<String>` | Yes | List of applicable hostnames |
| `secretRef` | `SecretObjectReference` | Yes | TLS certificate Secret reference |
| `minTlsVersion` | `TlsVersion?` | No | Minimum TLS version |
| `ciphers` | `Vec<String>?` | No | OpenSSL-format cipher suites (BoringSSL default when omitted) |

> **FIPS build note**: on a `fips`-feature gateway image, when `ciphers` is omitted the gateway
> defaults to the FIPS-approved TLS 1.2 cipher set (`ECDHE-ECDSA-AES128-GCM-SHA256`,
> `ECDHE-RSA-AES128-GCM-SHA256`, `ECDHE-ECDSA-AES256-GCM-SHA384`, `ECDHE-RSA-AES256-GCM-SHA384`)
> and ECDH curves (`P-256`, `P-384`, `P-521`) instead of the plain BoringSSL default. Setting
> `ciphers` explicitly always overrides this — the operator's list wins. On a **non-FIPS**
> build, the same approved default is also applied when `fips_policy` resolves to enforce
> (default `auto`: enabled when the host kernel reports FIPS mode via
> `/proc/sys/crypto/fips_enabled`, or explicit `on`) — this is cipher-suite policy alignment
> only, not a compliance claim. See
> `skills/04-review/edgion-tls/boring-v5-fips-build-verification.md` for the current contract.

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

## Relationship to Gateway TLS

| Feature | Gateway `tls` | EdgionTls |
|---------|---------------|-----------|
| Basic certificate binding | ✅ certificateRefs | ✅ secretRef |
| mTLS client authentication | ❌ | ✅ clientAuth |
| TLS version control | ❌ | ✅ minTlsVersion |
| Cipher suite control | ❌ | ✅ ciphers |

When using EdgionTls, set `tls.options.edgion.io/cert-provider: "edgion-tls"` on the Gateway Listener.
