# ACME Auto-Certificate (EdgionAcme)

> **🔌 Edgion Extension**
>
> `EdgionAcme` is an Edgion custom CRD for automatically requesting and renewing TLS certificates via the ACME protocol (e.g., Let's Encrypt), and automatically creating/updating EdgionTls and Secret resources.

## What is EdgionAcme?

EdgionAcme runs an ACME client on the Controller side, handling domain validation, certificate issuance and renewal, writing certificates to Kubernetes Secrets, and optionally auto-creating/updating EdgionTls resources for Gateway use.

**Key capabilities**:

- **HTTP-01**: Validates via HTTP access to `/.well-known/acme-challenge/<token>`, suitable for single domains or non-wildcard domains.
- **DNS-01**: Validates via DNS TXT records, **supports wildcard domains** (e.g., `*.example.com`).
- **Auto-renewal**: Automatically renews before expiry based on configuration, with exponential backoff retry on failure (up to 5 attempts).
- **EdgionTls integration**: Can automatically create or update EdgionTls after issuance, eliminating manual certificate resource maintenance.

**Current limitations**:

- Only **ECDSA** certificates are supported (ecdsa-p256 / ecdsa-p384).
- Does not interoperate with Kubernetes cert-manager; certificates and accounts are independently managed by the Edgion Controller.

---

## Quick Start

### Prerequisites

- Controller is deployed with cluster permissions to read/write EdgionAcme, Secret, and EdgionTls resources.
- Gateway is deployed; for HTTP-01, an HTTP listener (e.g., port 80) must be externally accessible, and the domain must resolve to that Gateway.

### Minimal Example (HTTP-01)

```yaml
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: example-acme
  namespace: default
spec:
  email: admin@example.com
  domains:
    - example.com
    - www.example.com
  challenge:
    type: http-01
    http01:
      gatewayRef:
        name: my-gateway
        namespace: default
  storage:
    secretName: example-tls-cert
  autoEdgionTls:
    enabled: true
    parentRefs:
      - name: my-gateway
        namespace: default
```

After creation, the Controller registers an account with the ACME service (default: Let's Encrypt), initiates an order, completes HTTP-01 validation, writes the certificate to the Secret specified by `storage.secretName`, and automatically creates/updates the EdgionTls resource.

---

## Configuration Reference

### Basic Fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `server` | string | No | Let's Encrypt production directory | ACME directory URL, e.g., `https://acme-v02.api.letsencrypt.org/directory`; use staging or Pebble for testing. |
| `email` | string | **Yes** | - | ACME account contact email. |
| `domains` | []string | **Yes** | - | Domains for certificate request; wildcards (e.g., `*.example.com`) require DNS-01. |
| `keyType` | string | No | `ecdsa-p256` | Certificate key type: `ecdsa-p256`, `ecdsa-p384`. |

### Challenge Method: challenge

| Field | Description |
|-------|-------------|
| `challenge.type` | `http-01` or `dns-01`. |
| `challenge.http01` | Required for HTTP-01, must specify `gatewayRef` (the Gateway used to serve challenge responses). |
| `challenge.dns01` | Required for DNS-01, must specify DNS provider and credentials. |

**HTTP-01**: The ACME service will access `http://<domain>/.well-known/acme-challenge/<token>`. The Controller writes the token to EdgionAcme and syncs it to the Gateway, which responds to that path only while there are pending challenges. After validation, the path is no longer intercepted.

**DNS-01**: The Controller creates a `_acme-challenge.<domain>` TXT record using the DNS provider API based on the value returned by ACME, waits for propagation, then notifies ACME for validation; the TXT record is deleted after success. Wildcard domains must use DNS-01.

### DNS-01 Configuration (dns01)

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `provider` | string | **Yes** | - | Supported: `cloudflare`, `alidns`; use `pebble` for testing (with Pebble + challtestsrv). |
| `credentialRef` | object | **Yes** | - | Secret reference containing DNS API credentials. |
| `propagationTimeout` | int | No | 120 | Maximum wait time for DNS propagation (seconds). |
| `propagationCheckInterval` | int | No | 5 | Propagation check interval (seconds). |

**Cloudflare**: The Secret must provide `api-token` (with Zone.DNS:Edit permission).

**Alibaba Cloud DNS (alidns)**: The Secret must provide `access-key-id` and `access-key-secret`.

### Renewal and Retry: renewal

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `renewBeforeDays` | int | 30 | Number of days before expiry to start renewal. |
| `checkInterval` | int | 86400 | Interval for periodic renewal checks (seconds), e.g., 24 hours. |
| `failBackoff` | int | 300 | Base delay for first retry after failure (seconds); uses exponential backoff (300, 1200, 4800...), up to 5 retries. |

### Certificate Storage: storage

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `secretName` | string | **Yes** | Name of the Secret storing `tls.crt` and `tls.key`. |
| `secretNamespace` | string | No | Namespace of the Secret; defaults to the EdgionAcme namespace. |

### Auto EdgionTls: autoEdgionTls

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | true | Whether to automatically create/update EdgionTls. |
| `name` | string | No | EdgionTls name; defaults to `acme-<EdgionAcme name>`. |
| `parentRefs` | []ParentRef | No | Gateway bindings for the auto-created EdgionTls. |

---

## Manual Trigger

When you need to immediately re-initiate a request or renewal (e.g., after fixing DNS/network issues), you can manually trigger processing, which has the same effect as "making the Controller reprocess the resource" and resets the failure retry counter.

### Method 1: Modify Resource (kubectl)

Make any modification to the EdgionAcme metadata (e.g., add an annotation), and the Controller will receive the change and reprocess the resource:

```bash
kubectl annotate edgionacme example-acme -n default edgion.io/trigger="$(date +%s)" --overwrite
```

### Method 2: Admin API

The Controller's Admin API provides a dedicated trigger endpoint:

```bash
curl -X POST "http://<controller-admin>:8080/api/v1/services/acme/<namespace>/<name>/trigger"
```

Example: Trigger the ACME flow for `example-acme` in the `default` namespace:

```bash
curl -X POST "http://localhost:8080/api/v1/services/acme/default/example-acme/trigger"
```

Response example:

```json
{
  "success": true,
  "data": "ACME check triggered for default/example-acme"
}
```

---

## Status and Troubleshooting

### Status Phases (status.phase)

| Phase | Description |
|-------|-------------|
| `Pending` | Certificate not yet issued, awaiting first issuance or retry. |
| `Issuing` | ACME order in progress (validating or issuing). |
| `Ready` | Certificate has been issued and is valid. |
| `Renewing` | Renewal in progress. |
| `Failed` | Last operation failed; retries with exponential backoff, up to 5 times. |

### Common Status Fields

- `status.certificateSerial`, `status.certificateNotAfter`: Current certificate serial number and expiry time (RFC 3339).
- `status.lastFailureReason`, `status.lastFailureTime`: Most recent failure reason and time.
- `status.secretName`, `status.edgionTlsName`: Current Secret and EdgionTls names in use.

### Common Issues

1. **HTTP-01 validation failure**  
   Confirm the domain resolves to the Gateway serving HTTP, and port 80 is accessible from the public internet; after the Controller writes the token, the Gateway must receive it via List-Watch and respond to `/.well-known/acme-challenge/<token>`.

2. **DNS-01 validation timeout**  
   Increase `dns01.propagationTimeout`; confirm the `credentialRef` Secret is correct and the DNS provider API is accessible.

3. **Certificate stuck in Failed state**  
   Check `status.lastFailureReason`; after fixing configuration or network issues, use the "Manual Trigger" method described above to re-initiate and reset the retry counter.

---

## Examples

- **HTTP-01**: See `examples/test/conf/Services/acme/http01-example.yaml` in the repository.
- **DNS-01 (Cloudflare)**: `examples/test/conf/Services/acme/dns01-cloudflare-example.yaml`.
- **DNS-01 (Alibaba Cloud)**: `examples/test/conf/Services/acme/dns01-alidns-example.yaml`.
- **Local Integration Test (Pebble)**: Docker Compose and instructions under `examples/test/conf/Services/acme/pebble/`.

For usage with EdgionTls, see the [EdgionTls User Guide](./edgion-tls.md).
