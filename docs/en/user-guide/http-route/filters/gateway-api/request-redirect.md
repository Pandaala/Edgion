# RequestRedirect

## Overview

`RequestRedirect` returns an HTTP redirect response (`301`, `302`, `303`, `307`, or `308`) instead of proxying the request to a backend. It is a standard Gateway API filter and runs in the request stage. Because it terminates the request, the upstream is never contacted. Use it to enforce HTTPS, move traffic to a new hostname, or rewrite paths during a migration.

## Configuration

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: redirect-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - filters:
        - type: RequestRedirect
          requestRedirect:
            scheme: https
            hostname: new.example.com
            port: 443
            statusCode: 301
            path:
              type: ReplaceFullPath
              replaceFullPath: /new-path
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `scheme` | Enum | none | Redirect scheme. Allowed values: `http` \| `https`; uses the incoming request scheme when omitted |
| `hostname` | String | none | Hostname to redirect to; keeps the original host when omitted |
| `port` | Integer | none | Port to redirect to (`1`–`65535`). When omitted, an explicit `scheme` selects its default port; otherwise the Gateway Listener port is preserved |
| `statusCode` | Enum | `302` | Redirect status. Allowed values: `301` \| `302` \| `303` \| `307` \| `308` |
| `path` | Object | none | Path rewrite to apply to the redirect target; keeps the original path when omitted |

### `path` sub-fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `path.type` | Enum | none | Allowed values: `ReplaceFullPath` \| `ReplacePrefixMatch` |
| `path.replaceFullPath` | String | none | Used when `type: ReplaceFullPath`; replaces the entire path |
| `path.replacePrefixMatch` | String | none | Used when `type: ReplacePrefixMatch`; replaces only the portion matched by the rule's `PathPrefix` |

## Examples

### Example 1: Force HTTPS

Redirect all plain-HTTP traffic to HTTPS with a permanent 301, keeping the original host and path.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: https-redirect
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: http
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /
      filters:
        - type: RequestRedirect
          requestRedirect:
            scheme: https
            port: 443
            statusCode: 301
```

### Example 2: Move a hostname and rewrite the path prefix

Send `/old-api/...` on the legacy host to `/v2/...` on the new host with a temporary 302.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: migrate-redirect
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - "legacy.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /old-api
      filters:
        - type: RequestRedirect
          requestRedirect:
            hostname: api.example.com
            statusCode: 302
            path:
              type: ReplacePrefixMatch
              replacePrefixMatch: /v2
```

## Notes

- Edgion validates this filter against a generated contract extracted from its bundled
  Gateway API CRD. Kubernetes rejects invalid values during admission; filesystem and
  etcd inputs report the same selected transform constraints through HTTPRoute status.
  An invalid rule is disabled without disabling valid sibling rules.
- A rule with `RequestRedirect` must omit `backendRefs`; the redirect is the terminal response.
- `301` and `308` are permanent redirects; `302` and `307` are temporary. `303` directs the client to retrieve the target (normally with GET). Use `307` or `308` when the client must preserve the request method and body; `301` and `302` can turn a POST into a GET.
- The `Location` header omits port `80` for HTTP and port `443` for HTTPS. Non-default ports are included.
- `RequestRedirect` and [URLRewrite](./url-rewrite.md) are mutually exclusive within the same rule. A rule that declares both is marked `PartiallyInvalid` and disabled, while the route's other rules keep working.
- A rule-level `RequestRedirect` is terminal and must not be combined with
  `backendRefs` in the same rule; the Kubernetes CRD rejects that shape.
- Because the filter terminates the request, any request-stage plugins declared after it do not run, and the backend is never reached.
- `ReplacePrefixMatch` replaces only the part of the path matched by the rule's `PathPrefix`; the remainder of the path is preserved. This modifier requires exactly one match in the rule, with path type `PathPrefix`.
- The original query string is always preserved and appended to the redirect `Location`. For example, `http://app.example.com/login?returnTo=/dashboard` redirects to `https://app.example.com/login?returnTo=/dashboard`. The Gateway API redirect filter has no query modifier, so the client's query always survives, regardless of any path modifier.

## Related documents

- [URLRewrite](./url-rewrite.md)
- [RequestHeaderModifier](./request-header-modifier.md)
- [HTTP to HTTPS redirect (gateway level)](../../../../ops-guide/gateway/http-to-https-redirect.md)
- [Plugin Composition and Execution Order](../plugin-composition.md)
