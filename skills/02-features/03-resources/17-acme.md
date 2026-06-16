---
name: acme-features
description: Full EdgionAcme CRD schema: Let's Encrypt automatic certificate issuance, renewal, DNS-01/HTTP-01 challenges.
---

# EdgionAcme Resource

> API: `edgion.io/v1` | Scope: Namespaced

EdgionAcme implements automatic certificate management via the ACME protocol (Let's Encrypt and similar), supporting HTTP-01 and DNS-01 challenges.

## Full Schema

```yaml
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: my-acme
  namespace: default
spec:
  server: "https://acme-v02.api.letsencrypt.org/directory"   # ACME server
  email: "admin@example.com"                                  # Required: contact email
  domains:                                                     # Required: certificate domain list
    - "example.com"
    - "*.example.com"                                          # Wildcard domains require DNS-01

  keyType: ecdsa-p256                                          # Certificate key algorithm

  challenge:                                                   # tagged enum: type=http-01 | dns-01
    type: http-01
    gatewayRef:                                                # Gateway reference required by HTTP-01
      name: my-gateway
      namespace: default
    # When switching to DNS-01, replace the entire section with:
    # type: dns-01
    # provider: cloudflare                                     # cloudflare | alidns
    # credentialRef:
    #   name: cloudflare-api-token
    #   namespace: default
    # propagationTimeout: 120
    # propagationCheckInterval: 5

  renewal:
    renewBeforeDays: 30                                        # Renew this many days before expiration
    checkInterval: 86400                                       # Renewal check interval (seconds)
    failBackoff: 300                                           # Backoff after failure (seconds)

  # externalAccountBinding:                                    # EAB (optional)
  #   keyId: "kid123"
  #   keySecretRef:                                            # Secret.data.hmacKey = base64url HMAC key
  #     name: eab-hmac-key
  #     namespace: default

  storage:
    secretName: "my-acme-cert"                                 # Secret name for storing the certificate
    secretNamespace: "default"                                 # Namespace of the certificate Secret

  autoEdgionTls:
    enabled: true                                              # Auto-create EdgionTls
    name: "acme-my-acme"                                       # EdgionTls name
    parentRefs:                                                # EdgionTls parentRefs
      - name: my-gateway
        sectionName: https
```

## spec Field Details

### Basic Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `server` | `String` | Let's Encrypt production | ACME directory URL |
| `email` | `String` | **Required** | ACME account registration email |
| `domains` | `Vec<String>` | **Required** | Certificate domain list |
| `keyType` | `AcmeKeyType` | `ecdsa-p256` | Key algorithm |

**AcmeKeyType enum**:

| Value | Description |
|-------|-------------|
| `ecdsa-p256` | ECDSA P-256 (default, recommended) |
| `ecdsa-p384` | ECDSA P-384 |

### challenge — Challenge Configuration

`challenge` is a `#[serde(tag = "type")]` internally tagged enum, corresponding to the Rust `AcmeChallenge`. `type` determines the variant, and the corresponding fields are flattened directly under `challenge`; there are no `http01:` / `dns01:` sub-fields. The two field groups are mutually exclusive at the type level.

**HTTP-01** (`type: http-01`):

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `gatewayRef` | `ParentReference` | Yes | Gateway used to respond to the ACME challenge |

**DNS-01** (`type: dns-01`):

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `provider` | `String` | — | DNS provider: `cloudflare` / `alidns` |
| `credentialRef` | `SecretObjectReference` | — | API credentials Secret |
| `propagationTimeout` | `u64` | `120` | DNS propagation timeout (seconds) |
| `propagationCheckInterval` | `u64` | `5` | Propagation check interval (seconds) |

### renewal — Renewal Configuration

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `renewBeforeDays` | `u32` | `30` | Number of days before expiration to trigger renewal |
| `checkInterval` | `u64` | `86400` | Renewal check interval (seconds, default 24h) |
| `failBackoff` | `u64` | `300` | Backoff after failure (seconds, default 5min) |

### storage — Certificate Storage

| Field | Type | Description |
|-------|------|-------------|
| `secretName` | `String` | Name of the K8s Secret storing the certificate |
| `secretNamespace` | `String?` | Secret namespace (defaults to the same as EdgionAcme) |

### autoEdgionTls — Auto EdgionTls

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | `bool` | `true` | Automatically create/update an EdgionTls resource |
| `name` | `String?` | `acme-{name}` | EdgionTls resource name |
| `parentRefs` | `Vec<ParentReference>?` | — | Gateway attachment points for the EdgionTls |

## Status Schema

```yaml
status:
  phase: Ready                                     # Pending | Issuing | Ready | Renewing | Failed
  certificateSerial: "1234567890"
  certificateNotAfter: "2026-06-20T00:00:00Z"      # Expiration time
  lastRenewalTime: "2026-03-20T12:00:00Z"
  lastFailureReason: ""
  lastFailureTime: ""
  secretName: "my-acme-cert"
  edgionTlsName: "acme-my-acme"
  accountUri: "https://acme-v02.api.letsencrypt.org/acme/acct/123"
  activeChallenges:                                # In-flight HTTP-01 challenge tokens
    - domain: "example.com"
      token: "..."
      keyAuthorization: "..."
      expireAt: 1700000000
```

`activeChallenges` is written by `AcmeService` via the `/status` subresource SSA (with a dedicated fieldManager `edgion-acme-challenges`). The Gateway reads it via the `EdgionAcme.active_challenges()` helper to serve `/.well-known/acme-challenge/<token>`. Non-empty only while HTTP-01 is in flight. Operators must not write it manually.

**AcmeCertPhase enum**:

| Phase | Description |
|-------|-------------|
| `Pending` | Waiting for the first issuance |
| `Issuing` | ACME issuance in progress |
| `Ready` | Certificate is valid |
| `Renewing` | Renewal in progress |
| `Failed` | Last operation failed |

## Operational Constraints

- The ACME service only runs on the **Leader** Controller.
- HTTP-01 challenges require port 80 of the Gateway to be reachable.
- Wildcard domains must use the DNS-01 challenge.
