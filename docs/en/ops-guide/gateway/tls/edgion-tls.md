# EdgionTls

> **🔌 Edgion extension**
>
> `EdgionTls` is an Edgion custom CRD (`edgion.io/v1`) that adds TLS capabilities beyond the Gateway API: mutual TLS (mTLS), a minimum TLS version, and an OpenSSL-format cipher list. It is not part of standard Gateway API.

EdgionTls binds a server certificate to one or more Gateway listeners and controls how the TLS handshake behaves. Use it when the standard Gateway API `tls.certificateRefs` is not enough — for example when you need client-certificate verification, want to pin a minimum TLS version, or must constrain the cipher suites.

## Overview

EdgionTls attaches to a Gateway listener via `parentRefs` and matches incoming connections by SNI against its `hosts` list. On a match, the Gateway uses the certificate from `secretRef` and applies the handshake options you configured:

```
Client --[TLS + SNI]--> Gateway listener
                          └─ EdgionTls (hosts match) -> serve secretRef cert,
                             enforce clientAuth / minTlsVersion / ciphers
```

For automatic certificate issuance and renewal, pair EdgionTls with [EdgionAcme](./acme.md) — ACME can create and keep an EdgionTls up to date for you.

## Quick Start

### Simplest HTTPS configuration

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
```

### Creating the certificate Secret

```bash
kubectl create secret tls example-com-tls \
  --cert=path/to/tls.crt \
  --key=path/to/tls.key \
  -n production
```

The Secret must be of type `kubernetes.io/tls` with `tls.crt` and `tls.key` keys. ⚠️ **Never store real private keys in version control or documentation.**

### Verify

```bash
kubectl get edgiontls -n production
curl -v https://example.com
```

## Configuration parameters

### Basic parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `parentRefs` | Array\<Object\> | none | Gateway listeners this config attaches to. Use `name` + `sectionName` to target a specific listener |
| `hosts` | Array\<String\> | **Required** | Hostnames this certificate serves. Supports a single left-most-label wildcard, e.g. `*.example.com` |
| `secretRef` | Object | **Required** | `kubernetes.io/tls` Secret holding the server certificate. Cross-namespace requires a `ReferenceGrant` |
| `clientAuth` | Object | none | mTLS client-certificate verification; see the table below |
| `minTlsVersion` | Enum | none (BoringSSL default = TLS 1.2+) | Minimum accepted TLS version. Allowed values: `TLS1_0` \| `TLS1_1` \| `TLS1_2` \| `TLS1_3` |
| `ciphers` | Array\<String\> | none (BoringSSL defaults) | OpenSSL-format cipher list for TLS 1.2 and below, e.g. `ECDHE-RSA-AES256-GCM-SHA384` |
| `extend` | Object | none | Reserved / experimental handshake knobs; see [Reserved parameters](#reserved-parameters) |

> There is **no** `maxTlsVersion` field, and ciphers are a flat OpenSSL-format list — there is no cipher "profile" selector. Configure only the fields listed above.

#### `minTlsVersion` values

| Value | TLS version | Recommendation |
|-------|-------------|----------------|
| `TLS1_0` | TLS 1.0 | Deprecated by RFC 8996 — legacy clients only, at your own risk |
| `TLS1_1` | TLS 1.1 | Deprecated by RFC 8996 — legacy clients only, at your own risk |
| `TLS1_2` | TLS 1.2 | Recommended baseline for modern deployments |
| `TLS1_3` | TLS 1.3 | Strongest; use when all clients support it |

When `minTlsVersion` is unset, BoringSSL's default (TLS 1.2+) applies, which is the safe path.

### Security parameters — `clientAuth` (mTLS)

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `clientAuth.mode` | Enum | `Terminate` | Verification mode. Allowed values: `Terminate` \| `Mutual` \| `OptionalMutual` |
| `clientAuth.caSecretRef` | Object | none | CA certificate Secret (must contain a `ca.crt` key). **Required when `mode` is `Mutual` or `OptionalMutual`** |
| `clientAuth.verifyDepth` | Integer | `1` | Certificate-chain verification depth (1–9). Increase for intermediate-CA chains |
| `clientAuth.allowedSans` | Array\<String\> | none | Allowlist of client-certificate SANs; supports wildcards |
| `clientAuth.allowedCns` | Array\<String\> | none | Allowlist of client-certificate Common Names (exact match) |

#### `clientAuth.mode` values

| Value | Meaning |
|-------|---------|
| `Terminate` | One-way TLS: only the server certificate is verified (default) |
| `Mutual` | Mutual TLS: the client **must** present a valid certificate or the handshake is rejected |
| `OptionalMutual` | The client certificate is optional, but if presented it is verified |

### Reserved parameters

The `extend` object holds handshake knobs that are **accepted by the schema but reserved for a future release**. They are validated and a config-time warning is logged, but they have **no runtime effect today**:

| Parameter | Status |
|-----------|--------|
| `extend.preferServerCiphers` | Reserved — no effect |
| `extend.ocspStapling` | **Not implemented** — no OCSP stapling occurs regardless of `enabled` (RFC 6960) |
| `extend.sessionTicket` | Reserved — no effect |
| `extend.sessionCache` | Reserved — no effect |
| `extend.revocationCheck` | **Not implemented** — `mode: ocsp` / `mode: crl` do **not** enable revocation checking; revoked certificates are still accepted |
| `extend.earlyData` | Reserved — no effect |

⚠️ **Do not rely on `extend.revocationCheck` or `extend.ocspStapling` for a compliance guarantee.** Setting them does not cause Edgion to reject revoked certificates today.

## Common configuration scenarios

### 1. One-way TLS (basic HTTPS)

The simplest production setup — verify only the server certificate, on multiple hosts:

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: basic-https
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: https
  hosts:
    - api.example.com
    - www.example.com
  secretRef:
    name: example-com-tls
    namespace: production
```

### 2. Required mutual TLS (`Mutual`)

Every client must present a certificate signed by the configured CA:

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: mtls-required
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: https
  hosts:
    - api.example.com
  secretRef:
    name: example-com-tls
    namespace: production
  clientAuth:
    mode: Mutual
    caSecretRef:
      name: client-ca
      namespace: production
    verifyDepth: 1
```

Create the CA Secret (the `ca.crt` key is required):

```bash
kubectl create secret generic client-ca \
  --from-file=ca.crt=path/to/client-ca.pem \
  -n production
```

Test it:

```bash
# With a client certificate (accepted)
curl --cert client.crt --key client.key https://api.example.com

# Without a certificate (rejected at the TLS handshake)
curl https://api.example.com
```

### 3. Optional mutual TLS (`OptionalMutual`)

Allow anonymous clients, but verify a certificate when one is presented — useful when an upstream service grants extra privileges to identified clients:

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: mtls-optional
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: https
  hosts:
    - secure.example.com
  secretRef:
    name: example-com-tls
    namespace: production
  clientAuth:
    mode: OptionalMutual
    caSecretRef:
      name: client-ca
      namespace: production
```

### 4. mTLS with a SAN / CN allowlist

On top of CA verification, restrict which clients are accepted by SAN or CN:

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: mtls-allowlist
  namespace: production
spec:
  parentRefs:
    - name: internal-gateway
      sectionName: https
  hosts:
    - admin.example.com
  secretRef:
    name: admin-example-com-tls
    namespace: production
  clientAuth:
    mode: Mutual
    caSecretRef:
      name: client-ca
      namespace: production
    allowedSans:
      - "client1.example.com"
      - "*.internal.example.com"
    allowedCns:
      - "AdminClient"
```

A client is accepted only if its certificate is issued by `client-ca` **and** its SAN/CN matches the allowlist. Prefer `allowedSans` over `allowedCns` — CN-based identity is deprecated.

### 5. Intermediate-CA chain (`verifyDepth`)

When client certificates are issued by an intermediate CA below the root, raise the verification depth:

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: mtls-intermediate-ca
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: https
  hosts:
    - enterprise.example.com
  secretRef:
    name: enterprise-example-com-tls
    namespace: production
  clientAuth:
    mode: Mutual
    caSecretRef:
      name: root-ca
      namespace: production
    # Allow one intermediate CA between leaf and root.
    verifyDepth: 2
```

Chain structure and depth:

```text
[Client cert] <- [Intermediate CA] <- [Root CA]
```

- `verifyDepth: 1` — direct issuance only (default)
- `verifyDepth: 2` — one intermediate CA
- `verifyDepth: 3` — two intermediate CAs

The CA Secret's `ca.crt` may hold a multi-certificate PEM bundle (root plus intermediates); every certificate in the bundle is loaded into the client trust store.

### 6. TLS 1.3 only

Pin the minimum version to TLS 1.3 for modern-client-only endpoints:

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: tls13-only
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: https
  hosts:
    - secure.example.com
  secretRef:
    name: secure-example-com-tls
    namespace: production
  minTlsVersion: TLS1_3
```

Clients that only support TLS 1.2 or lower will fail the handshake. Note that TLS 1.3 cipher suites are fixed by BoringSSL and cannot be changed with the `ciphers` field.

### 7. Custom cipher list (TLS 1.2)

Constrain the TLS 1.2 cipher suites using OpenSSL names:

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: custom-ciphers
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: https
  hosts:
    - secure.example.com
  secretRef:
    name: secure-example-com-tls
    namespace: production
  minTlsVersion: TLS1_2
  ciphers:
    - ECDHE-ECDSA-AES128-GCM-SHA256
    - ECDHE-RSA-AES128-GCM-SHA256
    - ECDHE-ECDSA-AES256-GCM-SHA384
    - ECDHE-RSA-AES256-GCM-SHA384
    - ECDHE-ECDSA-CHACHA20-POLY1305
    - ECDHE-RSA-CHACHA20-POLY1305
```

The `ciphers` field only affects TLS 1.2 and below. For TLS 1.3, the suites are fixed:
`TLS_AES_128_GCM_SHA256`, `TLS_AES_256_GCM_SHA384`, `TLS_CHACHA20_POLY1305_SHA256`.

### 8. Multi-Gateway binding

Serve the same certificate from more than one Gateway:

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: shared-tls
  namespace: production
spec:
  parentRefs:
    - name: public-gateway
      sectionName: https
    - name: internal-gateway
      sectionName: https
  hosts:
    - shared.example.com
  secretRef:
    name: shared-example-com-tls
    namespace: production
```

## Certificate management

### Rolling certificate rotation (recommended)

```bash
# 1. Create the new certificate Secret
kubectl create secret tls example-com-tls-new \
  --cert=new-cert.pem --key=new-key.pem -n production

# 2. Point EdgionTls at the new Secret
kubectl patch edgiontls example-tls -n production \
  --type=merge -p '{"spec":{"secretRef":{"name":"example-com-tls-new"}}}'

# 3. Confirm the new certificate is being served
curl -v https://example.com 2>&1 | grep "expire date"

# 4. Remove the old Secret
kubectl delete secret example-com-tls -n production
```

### In-place update

```bash
kubectl create secret tls example-com-tls \
  --cert=new-cert.pem --key=new-key.pem -n production \
  --dry-run=client -o yaml | kubectl apply -f -
```

For fully automated issuance and renewal, use [EdgionAcme](./acme.md).

## FAQ

### Q1: How do I inspect the certificate a Secret holds?

**A**: Extract and decode it with `kubectl` and `openssl`:

```bash
kubectl get secret example-com-tls -n production \
  -o jsonpath='{.data.tls\.crt}' | base64 -d > cert.pem
openssl x509 -in cert.pem -noout -text
```

### Q2: How many subdomain levels does a wildcard cover?

**A**: Exactly one label. `*.example.com` matches `api.example.com` but not the apex `example.com` and not `deep.sub.example.com`. Only a single left-most-label wildcard is a valid `hosts` entry — patterns like `*.*.example.com`, `foo*.example.com`, or `a.*.example.com` are rejected at config-load time with an `InvalidSanPattern` error (RFC 6125 §6.4.3).

### Q3: What is the difference between `Mutual` and `OptionalMutual`?

**A**: `Mutual` rejects the handshake when the client presents no (or an invalid) certificate. `OptionalMutual` lets anonymous clients through but still verifies a certificate when one is presented. Use `OptionalMutual` for mixed traffic where some clients are identified and others are not.

### Q4: Can I use a self-signed CA for mTLS?

**A**: Yes — that is common for development and internal services. Put the CA's `ca.crt` in the Secret referenced by `caSecretRef`. For public-facing server certificates, use a trusted CA (for example via [EdgionAcme](./acme.md) and Let's Encrypt).

### Q5: How do I allow only TLS 1.3?

**A**: Set `minTlsVersion: TLS1_3`. There is no `maxTlsVersion` field — pinning the minimum to TLS 1.3 effectively makes the listener TLS-1.3-only, because no higher version exists. Clients limited to TLS 1.2 or below will receive a handshake failure.

### Q6: Does the `extend` block (OCSP stapling, revocation, session tickets) do anything?

**A**: Not yet. Every field under `extend` is reserved schema; the gateway logs a config-time warning and otherwise ignores them. In particular, `extend.revocationCheck` does **not** reject revoked certificates and `extend.ocspStapling` does **not** staple an OCSP response. Do not treat them as a security control today.

### Q7: My mTLS handshake fails — where do I start?

**A**: Verify, in order: (1) the client actually sends `--cert`/`--key`; (2) the client certificate is signed by the CA in `caSecretRef` and its `ca.crt` key is present; (3) `verifyDepth` is high enough for the chain; (4) if you set `allowedSans`/`allowedCns`, the client certificate matches the allowlist.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| `Secret not found` | `secretRef` points at a missing Secret | Create the Secret or correct the reference |
| TLS handshake fails for a known-good cert | SNI host not in `hosts`, or wildcard depth too deep | Add the host to `hosts`; remember wildcards cover one label only |
| `Client certificate required` | `mode: Mutual` and the client sent none | Have the client send `--cert`/`--key`, or use `OptionalMutual` |
| CA verification fails | `caSecretRef` Secret missing the `ca.crt` key | Recreate the Secret with `--from-file=ca.crt=...` |
| Chain too long | `verifyDepth` too small for the intermediate chain | Increase `verifyDepth` (2 or 3) |
| `InvalidSanPattern` | A multi-label / partial / non-leftmost wildcard in `hosts` or `allowedSans` | Use a single leftmost-label wildcard such as `*.example.com` |
| Certificate has no effect | EdgionTls not attached to the listener | Check `parentRefs` (`name` + `sectionName`) |

## Get help

If you run into problems:

1. Check the FAQ and Troubleshooting sections above
2. Check the gateway logs
3. Open an issue on GitHub

## Related documents

- [TLS Termination](./tls-termination.md)
- [ACME automatic certificates](./acme.md)
- [HTTPS Listener](../listeners/https.md)
- [TLS Passthrough Listener](../listeners/https.md#passthrough---tls-passthrough)
