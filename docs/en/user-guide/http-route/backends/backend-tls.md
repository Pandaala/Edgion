# Backend TLS

Configure TLS connections from the Gateway to backend services using the standard Gateway API `BackendTLSPolicy` resource.

## Overview

By default the Gateway connects to backends over plaintext. `BackendTLSPolicy` tells the Gateway to upgrade the upstream leg to TLS, validate the backend certificate against a CA, and optionally present a client certificate for mutual TLS:

```
Client → [TLS] → Gateway → [TLS] → Backend
```

`BackendTLSPolicy` is a standard Gateway API resource (it is not an Edgion extension). Edgion processes it internally as `gateway.networking.k8s.io/v1`. Clusters that only serve the older `v1alpha3` API are accepted via a fallback and converted to the same internal model; if your cluster serves both, prefer `v1`.

## Configuring BackendTLSPolicy

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: BackendTLSPolicy
metadata:
  name: backend-tls
  namespace: production
spec:
  targetRefs:
    - group: ""
      kind: Service
      name: secure-backend
  validation:
    caCertificateRefs:
      - group: ""
        kind: Secret
        name: backend-ca
    hostname: backend.internal
```

## Configuration Reference

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `targetRefs` | Array\<Object\> | **Required** | Services this policy applies to |
| `targetRefs[].group` | String | `""` | API group; `""` for core (Service) |
| `targetRefs[].kind` | String | `Service` | Target kind; typically `Service` |
| `targetRefs[].name` | String | **Required** | Service name |
| `targetRefs[].sectionName` | String | none | Optional Service port name to scope the policy to a single port |
| `validation.hostname` | String | **Required** | Hostname used for TLS SNI and certificate validation |
| `validation.caCertificateRefs` | Array\<Object\> | none | CA references (Secret or ConfigMap). Mutually exclusive with `wellKnownCACertificates` |
| `validation.wellKnownCACertificates` | Enum | none | Set to `System` to trust the system CA bundle. Mutually exclusive with `caCertificateRefs` |
| `validation.subjectAltNames` | Array\<Object\> | none | SAN allowlist; if set, the backend certificate must match at least one entry |
| `validation.subjectAltNames[].type` | Enum | **Required** | Allowed values: `Hostname` \| `URI` |
| `validation.subjectAltNames[].hostname` | String | none | DNS name (when `type: Hostname`); supports wildcards (`*.example.com`) |
| `validation.subjectAltNames[].uri` | String | none | Full URI (when `type: URI`), e.g. a SPIFFE ID |

> One of `caCertificateRefs` or `wellKnownCACertificates` must be provided. Setting both is rejected (the policy's `Accepted` condition becomes `Invalid`).

### CA reference sources

| `kind` | Key read | Notes |
|--------|----------|-------|
| `Secret` | `ca.crt` | Standard TLS Secret holding the CA bundle |
| `ConfigMap` | `ca.crt` | Plain ConfigMap holding the PEM CA bundle |

A CA reference that does not exist or is missing the `ca.crt` key sets the `ResolvedRefs` condition to `InvalidCACertificateRef`.

## CA Certificate Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: backend-ca
  namespace: production
type: kubernetes.io/tls
data:
  ca.crt: <base64-encoded-ca-cert>
```

## Subject Alternative Name Validation

When `subjectAltNames` is configured, the Gateway validates the backend certificate's SAN during TLS handshake. The connection is rejected if no SAN matches.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: BackendTLSPolicy
metadata:
  name: backend-san-check
  namespace: production
spec:
  targetRefs:
    - group: ""
      kind: Service
      name: secure-backend
  validation:
    caCertificateRefs:
      - group: ""
        kind: Secret
        name: backend-ca
    hostname: backend.internal
    subjectAltNames:
      - type: Hostname
        hostname: backend.internal
      - type: URI
        uri: spiffe://cluster.local/ns/default/sa/backend
```

Matching rules:
- **Hostname**: exact match or wildcard (`*.example.com` matches single-level subdomains)
- **URI**: exact match (e.g. SPIFFE IDs)
- Any single match across all entries is sufficient
- Omitting `subjectAltNames` preserves default behavior (hostname-only validation against the CA)

## Mutual TLS to the backend

To present a client certificate to the backend (mTLS), reference a client-certificate Secret via the Edgion extension option `edgion.io/client-certificate-ref`. The Secret must live in the same namespace as the `BackendTLSPolicy`.

> **🔌 Edgion extension**
>
> `options.edgion.io/client-certificate-ref` is an Edgion-specific addition to `BackendTLSPolicy`; client certificates for backend mTLS are not part of standard Gateway API.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: BackendTLSPolicy
metadata:
  name: backend-mtls
  namespace: production
spec:
  targetRefs:
    - group: ""
      kind: Service
      name: secure-backend
  validation:
    caCertificateRefs:
      - group: ""
        kind: Secret
        name: backend-ca
    hostname: backend.internal
  options:
    # Client certificate Secret in the same namespace, used for gateway → backend mTLS
    edgion.io/client-certificate-ref: "backend-client-cert"
```

## Examples

### Example 1: Internal Service TLS with a private CA

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: BackendTLSPolicy
metadata:
  name: internal-tls
  namespace: production
spec:
  targetRefs:
    - group: ""
      kind: Service
      name: internal-api
  validation:
    caCertificateRefs:
      - group: ""
        kind: Secret
        name: internal-ca
    hostname: internal-api.svc.cluster.local
```

### Example 2: Trust the system CA bundle

Use `wellKnownCACertificates: System` when the backend presents a certificate issued by a publicly trusted CA (for example, an external HTTPS API exposed through an `ExternalName` Service).

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: BackendTLSPolicy
metadata:
  name: external-api-tls
  namespace: production
spec:
  targetRefs:
    - group: ""
      kind: Service
      name: external-api
  validation:
    hostname: api.example.com
    wellKnownCACertificates: System
```

## Status and conflicts

The policy reports status on the associated Gateway (resolved through a `Service → Route → Gateway` lookup), not on the policy itself. Watch the `Accepted` and `ResolvedRefs` conditions:

- `Accepted: Invalid` — validation failed (for example, both `caCertificateRefs` and `wellKnownCACertificates` set).
- `Accepted: Conflicted` — a higher-priority policy targets the same Service (and `sectionName`). When several policies target the same Service/port, the earliest `creationTimestamp` wins, breaking ties by `{namespace}/{name}`. Different `sectionName` values do not conflict.
- `ResolvedRefs: InvalidCACertificateRef` — a CA reference is missing or lacks the `ca.crt` key.

## Related Documentation

- [Service Reference](./service-ref.md)
- [mTLS Configuration](../../../ops-guide/infrastructure/mtls.md)
