---
name: backend-tls-policy-features
description: Full BackendTLSPolicy schema: upstream backend TLS validation and mTLS.
---

# BackendTLSPolicy Resource

> API: `gateway.networking.k8s.io/v1` | Scope: Namespaced
> Gateway API v1.5.0 resource; older clusters may still serve `v1alpha3`

BackendTLSPolicy defines the TLS connection configuration from the gateway to the backend, including CA validation and client certificates (mTLS). In addition to core Service targets, Edgion accepts direct AI `edgion.io/EdgionBackend` targets.

## Full Schema

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: BackendTLSPolicy
metadata:
  name: my-backend-tls
  namespace: default
spec:
  targetRefs:                                      # Target Service
    - group: ""
      kind: Service
      name: backend-service
      sectionName: https                           # Optional: specify a Service port name

  validation:
    hostname: "backend.internal.svc"               # Required: TLS SNI and certificate validation hostname
    caCertificateRefs:                             # CA certificate references (Secret or ConfigMap)
      - group: ""
        kind: ConfigMap                            # ConfigMap: read the ca.crt key
        name: backend-ca-cert
    subjectAltNames:                               # SAN allowlist (optional)
      - type: Hostname
        hostname: "backend.internal.svc"
      - type: URI
        uri: "spiffe://cluster.local/ns/default/sa/backend"
    wellKnownCACertificates: System                # Use system CAs (optional)

  options:                                         # Edgion extension options
    edgion.io/client-certificate-ref: "client-cert"          # Client certificate (same namespace)
```

## Multi-version Compatibility Strategy

- The controller's primary internal model is processed as `v1`.
- If a cluster only serves `v1alpha3`, the controller can fall back to watching the older version, converting to the unified internal type before entering handlers.
- It is not recommended to start two watchers for both `v1` and `v1alpha3` simultaneously.
- If a cluster only serves `v1`, older `v1alpha3` manifests must be migrated to `v1`.
- In FileSystem mode, both `v1` and `v1alpha3` file inputs are accepted, but they should be uniformly converted to the `v1` internal model at the parsing entry.

## spec Field Details

### targetRefs

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `group` | `String` | Yes | Typically `""` (core) |
| `kind` | `String` | Yes | Typically `Service` |
| `name` | `String` | Yes | Service name |
| `sectionName` | `String?` | No | Service port name |

### validation

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `hostname` | `String` | Yes | TLS SNI and certificate validation hostname |
| `caCertificateRefs` | `Vec<CACertRef>?` | No | CA certificate reference list (supports Secret and ConfigMap; ConfigMap reads the `ca.crt` key) |
| `subjectAltNames` | `Vec<SubjectAltName>?` | No | SAN allowlist |
| `wellKnownCACertificates` | `String?` | No | `System`: use system CA certificates |

**SubjectAltName**:

| Field | Type | Description |
|-------|------|-------------|
| `type` | `String` | `Hostname` / `URI` |
| `hostname` | `String?` | Required when type=Hostname |
| `uri` | `String?` | Required when type=URI |

### options — Edgion Extensions

| Key | Value Format | Description |
|-----|--------------|-------------|
| `edgion.io/client-certificate-ref` | `name` | Client certificate Secret (same namespace, used for gateway→backend mTLS) |

## Status Schema

The ancestorRef is automatically resolved through a three-step `Service → Route → Gateway` lookup, pointing to the associated Gateway (not the BackendTLSPolicy itself).

```yaml
status:
  ancestors:
    - ancestorRef:
        group: gateway.networking.k8s.io
        kind: Gateway
        name: my-gateway
        namespace: default
      controllerName: edgion.io/gateway-controller
      conditions:
        - type: Accepted
          status: "True"
          reason: Accepted
        - type: ResolvedRefs
          status: "True"
          reason: ResolvedRefs
```

### Accepted condition reasons

| Reason | Meaning | Priority |
|--------|---------|----------|
| `Invalid` | Validation failed (e.g., caCertificateRefs and wellKnownCACertificates are mutually exclusive) | 1 (highest) |
| `NoValidCACertificate` | All CA certificate references are invalid | 2 |
| `Conflicted` | A higher-priority policy exists for the same target | 3 |
| `Accepted` | Accepted normally | 4 |

### ResolvedRefs condition reasons

| Reason | Meaning | Priority |
|--------|---------|----------|
| `InvalidKind` | The caCertificateRef kind is not supported (not Secret/ConfigMap) | 1 (highest) |
| `InvalidCACertificateRef` | The CA ref does not exist or is missing the `ca.crt` key | 2 |
| `ResolvedRefs` | All references resolved | 3 |

### Conflict Detection

When multiple BackendTLSPolicies target the same Service (with the same sectionName), only one is Accepted per the Gateway API priority rules:
- Sort by `creationTimestamp` ascending (earlier-created wins).
- Tie-break by lexicographic order of `{namespace}/{name}`.
- Different `sectionName` values do not conflict.
