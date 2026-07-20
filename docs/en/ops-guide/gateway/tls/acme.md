# ACME Automatic Certificates

> **🔌 Edgion extension**
>
> `EdgionAcme` is an Edgion custom CRD (`edgion.io/v1`) that requests and renews TLS certificates over the ACME protocol (RFC 8555, e.g. Let's Encrypt) and can automatically create/update the matching EdgionTls and Secret. It is not part of standard Gateway API.

EdgionAcme runs an ACME client on the Controller: it registers an account, validates domain ownership, issues and renews certificates, writes them to a Kubernetes Secret, and optionally maintains an EdgionTls resource so the Gateway can serve them.

## Overview

```
EdgionAcme (Controller) --ACME order--> ACME server (Let's Encrypt)
   ├─ HTTP-01: token published in status -> Gateway serves /.well-known/acme-challenge/<token>
   ├─ DNS-01:  _acme-challenge TXT record created via provider API
   └─ on success: write cert to storage.secretName, update EdgionTls (autoEdgionTls)
```

**Challenge types**

- **HTTP-01** validates ownership over `http://<domain>/.well-known/acme-challenge/<token>`. Suitable for single, non-wildcard domains; the Gateway's HTTP listener must be publicly reachable.
- **DNS-01** validates over a DNS TXT record and is the **only** option for wildcard domains (e.g. `*.example.com`).

**Current limitations**

- Only **ECDSA** keys are supported (`ecdsa-p256` / `ecdsa-p384`).
- Independent of cert-manager — Edgion manages its own ACME account and certificates.

## Quick Start

### Prerequisites

- The Controller is deployed with permission to read/write EdgionAcme, Secret, and EdgionTls.
- For HTTP-01: a reachable HTTP listener (typically port 80) and DNS that resolves the domain to that Gateway.

### Minimal example (HTTP-01, staging)

Start against Let's Encrypt **staging** while you validate the flow — staging has high rate limits but issues untrusted certificates:

```yaml
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: example-acme
  namespace: production
spec:
  # Let's Encrypt STAGING — use for testing only (issues untrusted certs).
  server: "https://acme-staging-v02.api.letsencrypt.org/directory"
  email: admin@example.com
  privateKeySecretRef:
    name: example-acme-account
  domains:
    - example.com
    - www.example.com
  challenge:
    type: http-01
    gatewayRef:
      name: main-gateway
      namespace: production
  storage:
    secretName: example-com-tls
  autoEdgionTls:
    enabled: true
    parentRefs:
      - name: main-gateway
        sectionName: https
```

Once issuance succeeds, switch `server` to the production directory `https://acme-v02.api.letsencrypt.org/directory` (the default if you omit `server`) to obtain a trusted certificate.

After creation the Controller registers the ACME account (storing its key in `privateKeySecretRef`), runs the challenge, writes the certificate to `storage.secretName`, and creates/updates the EdgionTls.

## Configuration parameters

### Basic parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `server` | String | Let's Encrypt production directory | ACME directory URL. Use the staging URL for testing |
| `email` | String | **Required** | Contact email for ACME account registration |
| `privateKeySecretRef` | Object | **Required** | Secret that stores the ACME **account** private key. Edgion creates it if missing; existing credentials are reused |
| `domains` | Array\<String\> | **Required** | Domains to request. Wildcards (e.g. `*.example.com`) require DNS-01 |
| `keyType` | Enum | `ecdsa-p256` | Certificate key algorithm. Allowed values: `ecdsa-p256` \| `ecdsa-p384` |
| `challenge` | Object | **Required** | Challenge configuration; see [Challenge configuration](#challenge-configuration) |
| `renewal` | Object | (defaults below) | Renewal timing; see [Renewal](#renewal) |
| `externalAccountBinding` | Object | none | External Account Binding for providers that require it (e.g. ZeroSSL); see [External Account Binding](#external-account-binding) |
| `storage` | Object | **Required** | Where the issued certificate is stored; see [Certificate storage](#certificate-storage) |
| `autoEdgionTls` | Object | (enabled) | Auto-create/update an EdgionTls; see [Auto EdgionTls](#auto-edgiontls) |

> `privateKeySecretRef` (the ACME **account** key) and `storage.secretName` (the issued **certificate**) are two different Secrets. The account Secret is reused across renewals; the certificate Secret is overwritten on each issuance.

### Challenge configuration

`challenge` is a tagged enum keyed by `type`; the type-specific fields are flattened directly under `challenge` (there are no `http01:` / `dns01:` sub-objects).

**HTTP-01** (`type: http-01`):

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `gatewayRef` | Object | **Required** | Gateway whose HTTP listener serves the challenge response |

The Controller publishes the token in `status.activeChallenges`; the Gateway reads it and answers `http://<domain>/.well-known/acme-challenge/<token>` only while a challenge is in flight, then stops intercepting that path.

**DNS-01** (`type: dns-01`):

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `provider` | String | **Required** | DNS provider. Allowed values: `cloudflare` \| `alidns` |
| `credentialRef` | Object | **Required** | Secret holding the provider's API credentials |
| `propagationTimeout` | Duration | `120s` | Upper bound on the DNS propagation wait (GEP-2257 duration). The Controller polls the zone's authoritative nameservers and requests validation as soon as all TXT records are visible; if the deadline expires with a record still confirmed missing, the flow fails into the `failBackoff` retry path |
| `propagationCheckInterval` | Duration | `5s` | Poll cadence for the propagation check (GEP-2257 duration) |

The Controller creates a `_acme-challenge.<domain>` TXT record via the provider API, polls the zone's authoritative nameservers until the record is visible (recursive resolution is used only to discover the authoritative servers, avoiding negative-cache false negatives), asks ACME to validate, then deletes the record. If the authoritative nameservers cannot be reached at all (e.g. outbound DNS is firewalled), the Controller waits out the full `propagationTimeout`, logs a warning, and proceeds optimistically — equivalent to the previous fixed-wait behavior. The deadline is evaluated between poll rounds, so a round of hanging DNS queries may run slightly past it. Provider credential Secret keys:

- **Cloudflare** — `api-token` (with Zone.DNS:Edit permission)
- **Alibaba Cloud DNS (`alidns`)** — `access-key-id` and `access-key-secret`

### Renewal

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `renewal.renewBefore` | Duration | `720h` | Renew this far before expiry (720h = 30 days) |
| `renewal.checkInterval` | Duration | `24h` | Interval between renewal checks |
| `renewal.failBackoff` | Duration | `5m` | Base retry delay after a failure. Each consecutive failure multiplies the delay by 4 (`failBackoff * 4^attempt`, e.g. 5m, 20m, 80m, ~5.3h, ~21.3h). After 5 consecutive failures the resource stays `Failed` and stops retrying until you edit the CRD or [manually trigger](#manual-trigger) it (which resets the counter); a success also resets it |

### External Account Binding

Some ACME providers (for example ZeroSSL) require an External Account Binding (EAB) to associate the new account with an existing one:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `externalAccountBinding.keyId` | String | **Required (when present)** | EAB key identifier from the provider |
| `externalAccountBinding.keySecretRef` | Object | **Required (when present)** | Reference to a Kubernetes Secret whose `hmacKey` data entry holds the base64url-encoded HMAC key from the provider |

To create the Secret:

```bash
kubectl create secret generic eab-hmac-key \
  --from-literal=hmacKey="<base64url-encoded-hmac-key-from-provider>" \
  -n production
```

```yaml
externalAccountBinding:
  keyId: "your-eab-key-id"
  keySecretRef:
    name: eab-hmac-key
    namespace: production
```

Omit `externalAccountBinding` entirely for Let's Encrypt, which does not require it.

### Certificate storage

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `storage.secretName` | String | **Required** | Secret to hold the issued `tls.crt` and `tls.key` |
| `storage.secretNamespace` | String | EdgionAcme namespace | Namespace for the certificate Secret |

### Auto EdgionTls

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `autoEdgionTls.enabled` | Boolean | `true` | Create/update an EdgionTls for the issued certificate |
| `autoEdgionTls.name` | String | `acme-<EdgionAcme name>` | Name of the auto-created EdgionTls |
| `autoEdgionTls.parentRefs` | Array\<Object\> | none | Gateway listeners the auto-created EdgionTls attaches to |

## Common configuration scenarios

### 1. Wildcard certificate via DNS-01 (Cloudflare)

Wildcards can only be issued with DNS-01:

```yaml
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: wildcard-acme
  namespace: production
spec:
  email: admin@example.com
  privateKeySecretRef:
    name: wildcard-acme-account
  domains:
    - "*.example.com"
    - example.com
  keyType: ecdsa-p384
  challenge:
    type: dns-01
    provider: cloudflare
    credentialRef:
      name: cloudflare-api-token
      namespace: production
    propagationTimeout: 180s
  renewal:
    renewBefore: 336h
    checkInterval: 12h
  storage:
    secretName: wildcard-example-com-tls
  autoEdgionTls:
    enabled: true
    parentRefs:
      - name: main-gateway
        sectionName: https
```

Create the provider credential Secret:

```bash
kubectl create secret generic cloudflare-api-token \
  --from-literal=api-token=<cloudflare-api-token> \
  -n production
```

⚠️ The `<cloudflare-api-token>` value is a placeholder — never commit a real token.

### 2. Multiple domains on one certificate (HTTP-01)

A single EdgionAcme can cover several non-wildcard domains in one certificate (SAN list):

```yaml
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: multi-domain-acme
  namespace: production
spec:
  email: admin@example.com
  privateKeySecretRef:
    name: multi-domain-acme-account
  domains:
    - example.com
    - www.example.com
    - api.example.com
  challenge:
    type: http-01
    gatewayRef:
      name: main-gateway
      namespace: production
  storage:
    secretName: multi-domain-tls
  autoEdgionTls:
    enabled: true
    parentRefs:
      - name: main-gateway
        sectionName: https
```

Each domain must resolve to the Gateway and pass its own HTTP-01 challenge.

### 3. Production issuance (Let's Encrypt prod)

Omit `server` to use the production directory by default. **Mind the rate limits** — validate with staging first (see Quick Start), then apply the production resource:

```yaml
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: example-acme-prod
  namespace: production
spec:
  # server omitted -> defaults to Let's Encrypt production.
  email: admin@example.com
  privateKeySecretRef:
    name: example-acme-account
  domains:
    - example.com
  challenge:
    type: http-01
    gatewayRef:
      name: main-gateway
      namespace: production
  storage:
    secretName: example-com-tls
  autoEdgionTls:
    enabled: true
    parentRefs:
      - name: main-gateway
        sectionName: https
```

## Manual trigger

To immediately re-run issuance or renewal (for example after fixing DNS or network problems), trigger reprocessing — this also resets the failure retry counter.

### Method 1: annotate the resource

Any metadata change makes the Controller reprocess the resource:

```bash
kubectl annotate edgionacme example-acme -n production \
  edgion.io/trigger="$(date +%s)" --overwrite
```

### Method 2: Admin API

> **Authentication required**: all Controller Admin API calls need a Bearer token. Set `EDGION_TOKEN` to a token from `edgion-cli gen-token`, or use `edgion-cli acme trigger` directly. See [Federation RBAC — CLI Token RBAC](../../federation-rbac.md#cli-token-rbac).

```bash
curl -X POST "http://<controller-admin>:12101/api/v1/services/acme/<namespace>/<name>/trigger" \
  -H "Authorization: Bearer $EDGION_TOKEN"
```

A successful response:

```json
{
  "success": true,
  "data": "ACME check triggered for production/example-acme"
}
```

If the ACME service is not running on the contacted replica (for example one that does not hold leadership), the request is not queued and the failure is reported rather than a misleading success:

```json
{
  "success": false,
  "error": "ACME service not running; trigger for production/example-acme was not queued"
}
```

## Status and troubleshooting

### Status phases (`status.phase`)

| Phase | Meaning |
|-------|---------|
| `Pending` | No certificate yet — awaiting first issuance or retry |
| `Issuing` | ACME order in progress (validating or issuing) |
| `Ready` | Certificate issued and valid |
| `Renewing` | Renewal in progress |
| `Failed` | Last operation failed; retried with exponential backoff (`failBackoff * 4^attempt`) for up to 5 attempts, then stays `Failed` until the CRD is edited or a [manual trigger](#manual-trigger) resets the retry counter |

### Useful status fields

- `status.certificateSerial`, `status.certificateNotAfter` — serial and expiry (RFC 3339) of the current certificate
- `status.lastFailureReason`, `status.lastFailureTime` — most recent failure detail
- `status.secretName`, `status.edgionTlsName` — the Secret and EdgionTls in use
- `status.accountUri` — the registered ACME account URI

### Common issues

1. **HTTP-01 fails** — confirm the domain resolves to the Gateway and its HTTP port (typically 80) is reachable from the internet. The Gateway must receive the published token via List-Watch and answer `/.well-known/acme-challenge/<token>`.
2. **DNS-01 times out** — increase `propagationTimeout` (a generous value is cheap: the poll loop exits as soon as the records are visible); verify the `credentialRef` Secret keys are correct and the provider API is reachable. A "propagation not confirmed" failure means the zone's authoritative nameservers still did not serve the TXT record at the deadline.
3. **Stuck in `Failed`** — read `status.lastFailureReason`, fix the cause, then use [Manual trigger](#manual-trigger) to re-run and reset the retry counter.
4. **Wildcard request rejected** — wildcards require `challenge.type: dns-01`; HTTP-01 cannot validate them.
5. **Rate-limited by the ACME server** — switch to the staging directory while testing; production Let's Encrypt enforces strict per-domain limits.

## Get help

If you run into problems:

1. Check the Status and troubleshooting section above
2. Check the Controller logs (ACME runs on the leader Controller)
3. Open an issue on GitHub

## Related documents

- [EdgionTls extension](./edgion-tls.md)
- [TLS Termination](./tls-termination.md)
- [HTTPS Listener](../listeners/https.md)
- [Federation RBAC](../../federation-rbac.md)
