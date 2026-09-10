# HTTPRoute Filters

Filters let you modify or act on a request as it flows through an HTTPRoute rule — rewriting headers, redirecting, mirroring traffic, enforcing authentication, rate limiting, and more. Edgion supports two families of filters: the **standard Gateway API filters** and the **Edgion extension plugins** attached through `ExtensionRef`.

## The two filter families

### Standard Gateway API filters

These are part of the Gateway API specification and are configured inline in `rules[].filters` (or `rules[].backendRefs[].filters`) with a `type` field:

| Filter `type` | What it does | Stage |
|---------------|--------------|-------|
| `RequestHeaderModifier` | Set, add, or remove request headers before forwarding | Request |
| `ResponseHeaderModifier` | Set, add, or remove response headers before returning | Response |
| `RequestRedirect` | Return a 301/302/303/307/308 redirect instead of proxying | Request (terminating) |
| `URLRewrite` | Rewrite the request hostname and/or path before forwarding | Request |
| `RequestMirror` | Copy a fraction of traffic to a second backend (fire-and-forget) | Request (non-blocking) |

A minimal example that injects a header:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: example-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/
      filters:
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set:
              - name: X-Gateway
                value: edgion
      backendRefs:
        - name: api-service
          port: 8080
```

### Edgion extension plugins (via ExtensionRef)

Everything beyond the five standard filters — CORS, JWT/Basic/Key/LDAP/HMAC auth, rate limiting, IP restriction, mocking, proxy rewrite, DSL scripting, and more — is provided as an `EdgionPlugins` custom resource. You attach it to a route with the standard Gateway API `ExtensionRef` filter:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: api-guard
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: api-guard
  namespace: production
spec:
  requestPlugins:
    - type: Cors
      config:
        allowOrigins: "https://app.example.com"
```

The full plugin catalog lives under [Edgion extension plugins](./edgion-plugins/README.md).

## Filter scope

Filters can be declared at two scopes:

| Scope | Where | Applies to |
|-------|-------|-----------|
| Rule scope | `rules[].filters` | Every request matching the rule |
| BackendRef scope | `rules[].backendRefs[].filters` | Only requests routed to that backend |

BackendRef-scoped filters are useful for canary deployments — for example, adding an `X-Canary` header only on the requests sent to the canary backend.

A rule-level `RequestRedirect` is a terminal route action. Its rule must omit
`backendRefs`; the Kubernetes CRD rejects a rule that combines both.

## Mutual exclusion: URLRewrite vs RequestRedirect

`URLRewrite` and `RequestRedirect` are **mutually exclusive** and cannot appear in the same rule. The conflict is detected at three levels:

1. Both appear in `rule.filters`.
2. Both appear in one `backendRef.filters`.
3. One appears in `rule.filters` and the other in `backendRef.filters` (cross-level).

A rule that violates this is marked `PartiallyInvalid`: that rule is disabled, but the other valid rules in the HTTPRoute keep working.

## Execution order

When a request picks up filters from global config, the route rule, and a backendRef, they run in a defined precedence and across defined proxy phases. See [Plugin Composition and Execution Order](./plugin-composition.md) for the full model, including how `ExtensionRef` plugins interleave with standard filters.

## Related documents

- [Plugin Composition and Execution Order](./plugin-composition.md)
- [HTTPRoute Overview](../overview.md)
- [RequestHeaderModifier](./gateway-api/request-header-modifier.md)
- [Edgion extension plugins](./edgion-plugins/README.md)
