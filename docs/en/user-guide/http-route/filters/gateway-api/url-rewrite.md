# URLRewrite

## Overview

`URLRewrite` rewrites the request hostname and/or path before the request is forwarded to the upstream. It is a standard Gateway API filter and runs in the request stage. Unlike [RequestRedirect](./request-redirect.md), the rewrite is transparent to the client: the request is proxied to the backend with the new hostname/path, and no redirect response is returned.

## Configuration

```yaml
filters:
  - type: URLRewrite
    urlRewrite:
      hostname: internal.example.com
      path:
        type: ReplacePrefixMatch
        replacePrefixMatch: /v2
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `hostname` | String | none | Replaces the request `Host` header sent to the upstream |
| `path` | Object | none | Path rewrite to apply; see below |

### `path` sub-fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `path.type` | Enum | none | Allowed values: `ReplaceFullPath` \| `ReplacePrefixMatch` |
| `path.replaceFullPath` | String | none | Used when `type: ReplaceFullPath`; replaces the entire path |
| `path.replacePrefixMatch` | String | none | Used when `type: ReplacePrefixMatch`; replaces only the portion matched by the rule's `PathPrefix` |

## Examples

### Example 1: Rewrite the upstream hostname

Forward requests to a backend that expects a specific internal `Host`, without changing the path.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: host-rewrite
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - "api.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /
      filters:
        - type: URLRewrite
          urlRewrite:
            hostname: internal.example.com
      backendRefs:
        - name: api-service
          port: 8080
```

### Example 2: ReplacePrefixMatch vs ReplaceFullPath

`ReplacePrefixMatch` strips the matched prefix and substitutes a new one, preserving the rest of the path (so `/api/users/42` becomes `/v2/users/42`). `ReplaceFullPath` discards the original path entirely.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: path-rewrite
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    # /api/users/42  ->  /v2/users/42
    - matches:
        - path:
            type: PathPrefix
            value: /api
      filters:
        - type: URLRewrite
          urlRewrite:
            path:
              type: ReplacePrefixMatch
              replacePrefixMatch: /v2
      backendRefs:
        - name: api-service
          port: 8080
    # /healthz  ->  /status  (full replacement)
    - matches:
        - path:
            type: PathPrefix
            value: /healthz
      filters:
        - type: URLRewrite
          urlRewrite:
            path:
              type: ReplaceFullPath
              replaceFullPath: /status
      backendRefs:
        - name: api-service
          port: 8080
```

## Notes

- Edgion validates this filter against a generated contract extracted from its bundled
  Gateway API CRD. Kubernetes rejects invalid values during admission; filesystem and
  etcd inputs report the same selected transform constraints through HTTPRoute status.
  An invalid rule is disabled without disabling valid sibling rules.
- `URLRewrite` and [RequestRedirect](./request-redirect.md) are mutually exclusive within the same rule. A rule that declares both is marked `PartiallyInvalid` and disabled, while the route's other rules keep working.
- Rewrite values are literals; path parameters (`:name`) and template variables are not expanded. For dynamic, variable-driven rewrites use the [ProxyRewrite plugin](../edgion-plugins/proxy-rewrite.md).
- `ReplacePrefixMatch` only makes sense with a `PathPrefix` match; it replaces exactly the matched prefix segment.

## Related documents

- [RequestRedirect](./request-redirect.md)
- [ProxyRewrite plugin](../edgion-plugins/proxy-rewrite.md)
- [RequestHeaderModifier](./request-header-modifier.md)
- [Plugin Composition and Execution Order](../plugin-composition.md)
