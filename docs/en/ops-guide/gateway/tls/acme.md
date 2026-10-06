# ACME Automatic Certificates

> **🔌 Edgion extension**
>
> `EdgionAcme` is an Edgion custom CRD (`edgion.io/v1`) that requests and renews TLS certificates over the ACME protocol (RFC 8555, e.g. Let's Encrypt) and can automatically create/update the matching EdgionTls and Secret. It is not part of standard Gateway API.

EdgionAcme runs an ACME client on the Controller: it registers an account, validates domain ownership, issues and renews certificates, writes them to a Kubernetes Secret, and optionally maintains an EdgionTls resource so the Gateway can serve them.

Built-in ACME supports **HTTP-01 only**, with no DNS-01 or DNS provider API integration. For DNS-01 or wildcard issuance, use cert-manager or another external issuer and reference its standard TLS Secret through Gateway/EdgionTls. Edgion already loads and hot-reloads these Secrets; no EdgionAcme resource is needed for externally managed certificates.

## Overview

Certificate issuance and activation are separate: the Gateway loads the certificate after its Secret is updated. The leader Controller serializes scheduling per EdgionAcme, persists retry checkpoints in status, and rechecks both its Kubernetes Lease term and current resource authority before ACME and Kubernetes side effects. Leadership or resource changes cooperatively cancel obsolete attempts. Requests already accepted by the CA cannot be recalled, and writes across multiple Kubernetes resources are not transactional.

```
EdgionAcme (Controller) --ACME order--> ACME server (Let's Encrypt)
   ├─ HTTP-01: token published in status -> Gateway serves /.well-known/acme-challenge/<token>
   └─ on success: write cert to storage.secretName, update EdgionTls (autoEdgionTls)
```

**Challenge types**

- **HTTP-01** validates ownership over `http://<domain>/.well-known/acme-challenge/<token>`. Suitable for one or more non-wildcard domains; the Gateway's HTTP listener must be publicly reachable.

**Current limitations**

- Only **ECDSA** keys are supported (`ecdsa-p256` / `ecdsa-p384`).
- Built-in issuance manages its own ACME account; externally issued TLS Secrets, including cert-manager output, are also supported.

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
| `domains` | Array\<String\> | **Required** | Domains to request. Wildcard domains are not supported |
| `keyType` | Enum | `ecdsa-p256` | Certificate key algorithm. Allowed values: `ecdsa-p256` \| `ecdsa-p384` |
| `challenge` | Object | **Required** | Challenge configuration; see [Challenge configuration](#challenge-configuration) |
| `renewal` | Object | (defaults below) | Renewal timing; see [Renewal](#renewal) |
| `externalAccountBinding` | Object | none | External Account Binding for providers that require it (e.g. ZeroSSL); see [External Account Binding](#external-account-binding) |
| `storage` | Object | **Required** | Where the issued certificate is stored; see [Certificate storage](#certificate-storage) |
| `autoEdgionTls` | Object | (enabled) | Auto-create/update an EdgionTls; see [Auto EdgionTls](#auto-edgiontls) |

> `privateKeySecretRef` (the ACME **account** key) and `storage.secretName` (the issued **certificate**) are two different Secrets. The account Secret is reused across renewals; the certificate Secret is overwritten on each issuance.

`privateKeySecretRef` and `externalAccountBinding.keySecretRef` accept `name` and
optional `namespace`, `group` and `kind`. Only core Kubernetes Secrets are
supported: omit `group` or set it to `""`, and
omit `kind` or set it to `Secret`. Omitted `namespace` uses the EdgionAcme
namespace. Other groups or kinds produce `Accepted=False` and
`ResolvedRefs=False` with reason `InvalidKind`, and block account restoration,
registration, and credential Secret reads/writes. A same-name Secret is never
used as a substitute for an unsupported target type.

### Challenge configuration

`challenge` accepts only `type: http-01` and a required `gatewayRef`. Other challenge types and unknown fields are rejected.

**HTTP-01** (`type: http-01`):

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `gatewayRef` | Object | **Required** | Gateway HTTP listener attachment that serves the challenge response; `namespace`, `sectionName`, and `port` may narrow the target |

Only selected plain-HTTP listeners can answer. If both `sectionName` and `port`
are present, both must match. A cross-namespace reference requires a
`ReferenceGrant` from `edgion.io/EdgionAcme` to
`gateway.networking.k8s.io/Gateway`; `allowedRoutes` is unrelated.

The Controller publishes the token in `status.activeChallenges`; the Gateway answers `http://<domain>/.well-known/acme-challenge/<token>` only while the challenge, selected listener, and current Gateway generation all match, then stops intercepting that path.

### Renewal

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `renewal.renewBefore` | Duration | `720h` | Renew this far before expiry (720h = 30 days) |
| `renewal.checkInterval` | Duration | `24h` | Interval between renewal checks |
| `renewal.failBackoff` | Duration | `5m` | Base retry delay after a failure. Each reserved attempt multiplies the delay by 4 (`failBackoff * 4^attempt`, e.g. 5m, 20m, 80m, ~5.3h, ~21.3h). After 5 attempts for one spec generation the resource stays `Failed`. A spec change starts a new generation; an explicit [Admin trigger](#manual-trigger) resets the current generation's budget; success clears it |

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

### 1. Multiple domains on one certificate (HTTP-01)

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

### 2. Production issuance (Let's Encrypt prod)

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

To immediately re-run issuance or renewal (for example after fixing DNS or network problems), use the Admin trigger. It has explicit reset semantics: the current spec generation's persisted retry budget is replaced before the next attempt. A metadata-only annotation is not an issuance command and does not reset the budget.

### Admin API

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
| `Failed` | Last operation failed; retried with exponential backoff for up to 5 reserved attempts in one spec generation, then stays `Failed` until the spec changes or an [Admin trigger](#manual-trigger) resets the retry counter |

### Useful status fields

- `status.certificateSerial`, `status.certificateNotAfter` — serial and expiry (RFC 3339) of the current certificate
- `status.lastFailureReason`, `status.lastFailureTime` — most recent failure detail
- `status.secretName`, `status.edgionTlsName` — the Secret and EdgionTls in use
- `status.accountUri` — the registered ACME account URI
- `status.scheduler` — read-only recovery checkpoint containing the observed spec generation, reserved attempt ID/count, attempt start time, and next retry time

### Common issues

1. **HTTP-01 fails** — confirm the domain resolves to the Gateway and its HTTP port (typically 80) is reachable from the internet. The Gateway must receive the published token via List-Watch and answer `/.well-known/acme-challenge/<token>`.
2. **Stuck in `Failed`** — read `status.lastFailureReason`, fix the cause, then use [Manual trigger](#manual-trigger) to re-run and reset the retry counter.
3. **Wildcard request rejected** — built-in HTTP-01 cannot validate wildcards. Use cert-manager or another external issuer and reference its TLS Secret.
4. **Rate-limited by the ACME server** — switch to the staging directory while testing; production Let's Encrypt enforces strict per-domain limits.

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
