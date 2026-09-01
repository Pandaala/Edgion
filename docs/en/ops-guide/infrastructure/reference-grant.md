# Cross-Namespace Reference (ReferenceGrant)

`ReferenceGrant` is a standard Gateway API resource that explicitly authorizes cross-namespace references. By default, Gateway API forbids a resource in one namespace from referencing a resource in another; a `ReferenceGrant` in the *target* namespace grants the exception, avoiding the security risk of unrestricted cross-namespace access.

## Overview

The grant always lives in the namespace of the resource being referenced (the **target**). It declares which `from` resources (by group, kind, and namespace) may reference which `to` resources (by group and kind) in that namespace.

```
namespace A (source)              namespace B (target)
  Gateway  ──── references ────►  Secret
                                  ReferenceGrant  ← lives here, allows the reference
```

> **API version**: Edgion's installed Gateway API CRDs serve ReferenceGrant at
> `gateway.networking.k8s.io/v1`; use that version in Edgion manifests.

## Typical scenarios

- A Gateway in `gateway-system` references a TLS certificate Secret in the `security` namespace.
- An HTTPRoute in a business namespace references a backend Service in a shared namespace.

## Example: allow a Gateway to reference a cross-namespace Secret

The Secret lives in `security`, so the grant is created in `security`:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: ReferenceGrant
metadata:
  name: allow-gw-to-secret
  namespace: security
spec:
  from:
    - group: gateway.networking.k8s.io
      kind: Gateway
      namespace: gateway-system
  to:
    - group: ""
      kind: Secret
```

## Example: allow EdgionTls to reference a cross-namespace Secret

`EdgionTls` resolves both `spec.secretRef` (server certificate) and
`spec.clientAuth.caSecretRef` (mTLS CA) through the same ReferenceGrant check. A
cross-namespace `secretRef` is rejected (the certificate stays unresolved and the
`ResolvedRefs` condition reports `RefNotPermitted`) unless a matching grant exists:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: ReferenceGrant
metadata:
  name: allow-edgiontls-to-secret
  namespace: security
spec:
  from:
    - group: edgion.io
      kind: EdgionTls
      namespace: gateway-system
  to:
    - group: ""
      kind: Secret
```

## Example: allow an HTTPRoute to reference a cross-namespace Service

The Service lives in `shared`, so the grant is created in `shared`:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: ReferenceGrant
metadata:
  name: allow-route-to-service
  namespace: shared
spec:
  from:
    - group: gateway.networking.k8s.io
      kind: HTTPRoute
      namespace: production
  to:
    - group: ""
      kind: Service
```

## Example: allow a Gateway to reference ProxyProtocolTrust data

A Gateway may select a typed inbound PROXY protocol trust policy through
`edgion.io/inbound-proxy-protocol`:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: edge
  namespace: gateway-system
  annotations:
    edgion.io/inbound-proxy-protocol: >-
      {"source":"resourceRef","resourceRef":{"name":"edge-load-balancers","namespace":"networking"}}
```

The target is an enabled, non-overlay `ProxyProtocolTrust` EdgionConfigData:

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: edge-load-balancers
  namespace: networking
spec:
  enable: true
  visibility: Namespace
  data:
    type: ProxyProtocolTrust
    config:
      mode: trustedSources
      trustedCidrs: [10.0.0.0/8]
```

Because the target is cross-namespace and has `visibility: Namespace`, this exact grant must live
in `networking`:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: ReferenceGrant
metadata:
  name: allow-edge-proxy-trust
  namespace: networking
spec:
  from:
    - group: gateway.networking.k8s.io
      kind: Gateway
      namespace: gateway-system
  to:
    - group: edgion.io
      kind: EdgionConfigData
      name: edge-load-balancers
```

With reference validation enabled, same-namespace targets are always allowed and cross-namespace
targets with `visibility: Cluster` are allowed without a grant. Only cross-namespace
`visibility: Namespace` targets require the exact grant above. Disabling Controller reference
validation allows all three cases.

A missing or mismatched group, kind, namespace, target kind, or optional target name denies the
reference. The Gateway reports `ResolvedRefs=False/RefNotPermitted`; the selected listener reports
`ResolvedRefs=False/InvalidProxyProtocolRef`; `Programmed` remains false. The Controller still
syncs an explicit Invalid transport state, so a Gateway process receiving that snapshot fails
startup closed rather than widening trust.

## Field reference

| Field | Type | Description |
|-------|------|-------------|
| `spec.from[].group` | String | API group of the referencing resource (e.g. `gateway.networking.k8s.io`) |
| `spec.from[].kind` | String | Kind of the referencing resource (e.g. `Gateway`, `HTTPRoute`) |
| `spec.from[].namespace` | String | Namespace the referencing resource must be in |
| `spec.to[].group` | String | API group of the target resource. Use `""` (empty string) for core resources like `Secret` and `Service` |
| `spec.to[].kind` | String | Kind of the target resource. Optionally pair with `name` to restrict to a single object |

> The core API group (Secret, Service, ConfigMap) is the empty string `""`, not `core`.

## Security recommendations

1. Keep `from` as narrow as possible — specify the exact namespace and kind.
2. Include only the kinds you actually need in `to`.
3. Use the optional `to[].name` field to restrict a grant to a single named object when feasible.
4. Audit cross-namespace authorizations regularly.

## FAQ

### Q1: My cross-namespace reference is rejected even though I created a ReferenceGrant. Why?

**A**: Confirm the grant is in the **target** namespace (the namespace of the resource being referenced), not the source namespace. Also confirm the `from` group/kind/namespace exactly matches the referencing resource, and that `to.group` is `""` for core resources.

### Q2: Do I need a ReferenceGrant for same-namespace references?

**A**: No. References within a single namespace are always allowed; ReferenceGrant is only required when source and target are in different namespaces.

### Q3: Why is my ReferenceGrant rejected by the API server?

**A**: Use `apiVersion: gateway.networking.k8s.io/v1`, matching the Gateway API CRDs installed by Edgion.

## Related documents

- [Secret Management](./secret-management.md)
- [mTLS Configuration](./mtls.md)
- [GatewayClass Configuration](../gateway/gateway-class.md)
