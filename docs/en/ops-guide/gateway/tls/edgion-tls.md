# EdgionTls

## Inherited TLS defaults

`spec.minTlsVersion` and `spec.ciphers` override the GatewayClass EGC defaults per identity.
An absent field inherits; an explicit `ciphers: []` selects backend defaults, even under FIPS policy.
There is no per-identity curve override. See [downstream TLS defaults](../../configuration/edgion-gateway-config.md).


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

To make a listener resolve its certificate from EdgionTls instead of the standard `tls.certificateRefs`, set the listener option `edgion.io/cert-provider: "edgion-tls"` on the Gateway. See [Selecting EdgionTls on a listener](#selecting-edgiontls-on-a-listener).

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

### Selecting EdgionTls on a listener

EdgionTls attaches through `parentRefs`, but the listener itself must opt in to using EdgionTls as its certificate source. Set the listener option `edgion.io/cert-provider` to `edgion-tls` so the Gateway resolves the certificate (and handshake options) from the matching EdgionTls rather than from the standard `tls.certificateRefs`:

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
        # Resolve the certificate and handshake options from EdgionTls.
        options:
          edgion.io/cert-provider: "edgion-tls"
        # certificateRefs may be empty when EdgionTls supplies the certificate.
        certificateRefs: []
```

## Configuration parameters

### Basic parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `parentRefs` | Array\<Object\> | none | Gateway listeners this config attaches to. Use `name` + `sectionName` to target a specific listener |
| `hosts` | Array\<String\> | **Required** | Hostnames this certificate serves. Supports a single left-most-label wildcard, e.g. `*.example.com` |
| `secretRef` | Object | **Required** | `kubernetes.io/tls` Secret holding the server certificate. Cross-namespace requires a `ReferenceGrant` |
| `clientAuth` | Object | none | mTLS client-certificate verification; see the table below |
| `minTlsVersion` | Enum | none (inherit EGC, then backend) | Minimum accepted TLS version. Allowed values: `TLS1_0` \| `TLS1_1` \| `TLS1_2` \| `TLS1_3` |
| `ciphers` | Array\<String\> | none (inherit EGC, then FIPS/backend) | OpenSSL-format cipher list for TLS 1.2 and below, e.g. `ECDHE-RSA-AES256-GCM-SHA384` |

> There is **no** `maxTlsVersion` field, and ciphers are a flat OpenSSL-format list — there is no cipher "profile" selector. Configure only the fields listed above.

#### `minTlsVersion` values

| Value | TLS version | Recommendation |
|-------|-------------|----------------|
| `TLS1_0` | TLS 1.0 | Deprecated by RFC 8996 — legacy clients only, at your own risk |
| `TLS1_1` | TLS 1.1 | Deprecated by RFC 8996 — legacy clients only, at your own risk |
| `TLS1_2` | TLS 1.2 | Recommended baseline for modern deployments |
| `TLS1_3` | TLS 1.3 | Strongest; use when all clients support it |

When `minTlsVersion` is unset, the EGC minimum applies; if both are absent, the backend default applies.

### Security parameters — `clientAuth` (mTLS)

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `clientAuth.mode` | Enum | `Terminate` | Verification mode. Allowed values: `Terminate` \| `Mutual` \| `OptionalMutual` |
| `clientAuth.caSecretRef` | Object | none | CA certificate Secret (must contain a `ca.crt` key). **Required when `mode` is `Mutual` or `OptionalMutual`** |
| `clientAuth.verifyDepth` | Integer | `1` | Certificate-chain verification depth (1–9). Increase for intermediate-CA chains |
| `clientAuth.allowedSans` | Array\<object\> | none | Typed SAN allowlist. Each entry has `type` (`DNS` \| `URI` \| `Email` \| `IP` \| `OtherName`) and `value`. Optional `match` (`Exact` default, `Prefix`, `Suffix`, `Contains`, `RegularExpression`), `ignoreCase`, and `oid` (required for `OtherName`). Omit or empty means no extra SAN constraint. DNS Exact uses RFC 6125 wildcards. RegularExpression is compiled at config load. |
| `clientAuth.allowedCns` | Array\<String\> | none | Allowlist of client-certificate Common Names (exact match) |

`allowedSans[].ignoreCase` defaults to `false`. For `Exact`, `Prefix`, `Suffix`,
and `Contains` string comparisons, `true` ignores ASCII case. DNS `Exact` always
uses RFC 6125 hostname matching; IP `Exact` compares parsed addresses.

For `RegularExpression`, `ignoreCase` sets the default case mode using Unicode
simple case folding. Inline `(?i)` and `(?-i)` flags override it within their
scope: `(?i)^spiffe://cluster$` is case-insensitive even with `ignoreCase: false`,
while `^spiffe://(?-i:cluster)$` with `ignoreCase: true` permits `SPIFFE://cluster`
but rejects `SPIFFE://CLUSTER`. Matching still requires the configured SAN type
and, for `OtherName`, the configured OID. Regexes compile at configuration load
under a 1,024-byte source limit and 1 MiB program/DFA limits. An invalid or
uncompiled entry in the active allowlist rejects the client. An invalid
configuration update may retain the previous valid TLS configuration, including
its mTLS policy, under the existing last-known-good rules.

#### `clientAuth.mode` values

| Value | Meaning |
|-------|---------|
| `Terminate` | One-way TLS: only the server certificate is verified (default) |
| `Mutual` | Mutual TLS: the client **must** present a valid certificate or the handshake is rejected |
| `OptionalMutual` | The client certificate is optional, but if presented it is verified |

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
      - type: DNS
        value: "client1.example.com"
      - type: DNS
        value: "*.internal.example.com"
    allowedCns:
      - "AdminClient"
```

A client is accepted only if its certificate is issued by `client-ca` **and** its SAN/CN matches the allowlist. Prefer `allowedSans` over `allowedCns` — CN-based identity is deprecated.

To pass the verified client-certificate identity downstream (to backends or plugins), add the annotation `edgion.io/expose-client-cert: "true"` to the EdgionTls metadata. After a successful mTLS handshake, the Gateway exposes the client certificate information for downstream use:

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: mtls-allowlist
  namespace: production
  annotations:
    edgion.io/expose-client-cert: "true"
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
```

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

### Q6: My mTLS handshake fails — where do I start?

**A**: Verify, in order: (1) the client actually sends `--cert`/`--key`; (2) the client certificate is signed by the CA in `caSecretRef` and its `ca.crt` key is present; (3) `verifyDepth` is high enough for the chain; (4) if you set `allowedSans`/`allowedCns`, the client certificate matches the allowlist.

### Q7: Can a TLS session established on one host be resumed on another host of the same listener?

**A**: Only when both hosts select the same effective TLS policy: the same EdgionTls resource or Gateway listener, the same serving certificate, and the same effective client authentication. A session from any other policy — for example a one-way TLS host — is not resumed on an mTLS host; the gateway performs a full handshake and requests the client certificate. Changing the serving certificate, or, for `Mutual`/`OptionalMutual`, the client CA bundle, `mode`, `verifyDepth`, `allowedSans` or `allowedCns`, also ends resumption of earlier sessions; on a Gateway listener with frontend client-certificate validation, changing its CA certificates or validation mode does the same, so the next connection performs one full handshake. Settings that have no runtime effect do not: on a host served by the EdgionTls alone, `Terminate` behaves like no `clientAuth`, and an empty allowlist behaves like an absent one. Resumption does not re-verify the client certificate: within an unchanged policy, an already-authenticated session can resume until it expires even if the client certificate has expired since. Sessions last at most 2 hours for TLS 1.2 and at most 7 days for TLS 1.3 (renewed on each resumption up to that cap), and are not shared across Gateway restarts, replicas or listeners.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| `Secret not found` | `secretRef` points at a missing Secret | Create the Secret or correct the reference |
| TLS handshake fails for a known-good cert | SNI host not in `hosts`, or wildcard depth too deep | Add the host to `hosts`; remember wildcards cover one label only |
| `Client certificate required` | `mode: Mutual` and the client sent none | Have the client send `--cert`/`--key`, or use `OptionalMutual` |
| CA verification fails | `caSecretRef` Secret missing the `ca.crt` key | Recreate the Secret with `--from-file=ca.crt=...` |
| Chain too long | `verifyDepth` too small for the intermediate chain | Increase `verifyDepth` (2 or 3) |
| `InvalidSanPattern` | Invalid `hosts` wildcard, or an `allowedSans` entry that fails type/match/regex/OID checks | DNS Exact values must be Gateway API hostnames (`*.example.com` is allowed). Compile RegularExpression at apply time; OtherName requires `oid`. |
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
- [TCP / TLS Listener](../listeners/tcp.md)
