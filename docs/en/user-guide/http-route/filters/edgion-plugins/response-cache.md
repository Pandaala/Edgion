# ResponseCache

> **Edgion extension**
> ResponseCache reuses selected finite public origin responses through the
> EdgionPlugins CRD. It is not a standard Gateway API filter.

## Quick start

The origin must return an explicit `Cache-Control: public, max-age=...` response
with valid freshness metadata. Add the plugin to a route with one Service backend:

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: public-cache
  namespace: default
spec:
  requestPlugins:
    - type: ResponseCache
      config:
        maxTtl: 60s
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: public-cache
  namespace: default
spec:
  parentRefs:
    - name: public-gateway
  hostnames: ["public.example.com"]
  rules:
    - matches:
        - method: GET
          path:
            type: PathPrefix
            value: /public
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: public-cache
      backendRefs:
        - name: public-origin
          port: 80
```

Use an existing Gateway `public-gateway` and Service `public-origin` in this
namespace. Authentication and quotas still run for every request before lookup.
Hits run current response-header processing and receive a current Age value.

Requests containing `Range`, `If-Range`, `If-Match`, `If-None-Match`,
`If-Modified-Since`, or `If-Unmodified-Since` bypass cache lookup and capture.
The origin handles range and conditional request semantics.

## Configuration parameters

| Parameter | Type | Default | Description |
| --- | --- | --- | --- |
| `maxTtl` | Duration | `60s` | Upper freshness limit; positive and at most 24 hours. Origin freshness may shorten it. |
| `graphqlQueries` | Boolean | `false` | Permit validated finite POST GraphQL queries using the final raw body identity. |
| `vary` | Header KeyGet list | `[]` | At most 16 non-sensitive header selectors; also represents absent values for origin Vary. |

Unknown configuration fields reject validation. Physical process limits belong
to the Gateway's local `response_cache` configuration, rather than this plugin.
Defaults retain at most 64 entries with 1 MiB bodies, 16 KiB metadata, 16 MiB
concurrent pending fills and 128 MiB total cache ownership. Admission refusal
continues ordinary origin forwarding.

## Common configuration scenarios

### Short-lived public documents

Attach this plugin as in the quick-start route. Responses without explicit public
freshness, including no-store/private responses, continue directly from origin.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: public-cache
  namespace: default
spec:
  requestPlugins:
    - type: ResponseCache
      config:
        maxTtl: 10s
```

### Header variation

The following policy supports an origin `Vary: x-public-language`, including the
absence of that header. Effective end-to-end headers already distinguish values;
the selector explicitly represents absence. Unsupported Vary values bypass cache.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: public-cache
  namespace: default
spec:
  requestPlugins:
    - type: ResponseCache
      config:
        maxTtl: 60s
        vary:
          - type: header
            name: x-public-language
```

### Validated GraphQL queries

Put GraphqlGuard before ResponseCache and attach this policy to a POST GraphQL
route. The selected operation must be a successfully inspected query, the final
body must be captured and physically complete, and its size must not exceed
64 KiB. Mutations, subscriptions and invalid queries do not become cache entries.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: public-cache
  namespace: default
spec:
  requestPlugins:
    - type: GraphqlGuard
      config:
        maxDepth: 8
        maxFields: 64
    - type: ResponseCache
      config:
        maxTtl: 30s
        graphqlQueries: true
```

Identity uses the final raw body bytes and selected operation; numeric values
are not rounded through a JSON number representation. Final rewrites and header
changes participate in identity before lookup.

## Supported combinations and security

Authorization and Cookie requests bypass. The initial supported KeyAuth profile
uses only header credentials, removes them, has no anonymous principal and emits
no upstream consumer fields. The origin response must be public for every allowed
identity in that profile. Other authentication profiles conservatively bypass.

Response-body processors, WAF, protocol conversions, gRPC, WebSocket, SSE,
streaming or trailer responses, multiple backends, backend filters and unsupported
runtime companions bypass. Configured dynamic upstream-authority templates also
bypass: their required variables are outside the initial cache profile.
Sensitive, hop-by-hop, framing and managed tracing headers cannot be added through
`vary`. Keys and response payloads are absent from cache diagnostics.

## Reloads and failures

Configuration publication broadly invalidates visible entries and fences pending
fills. A response exceeding its bound still forwards normally and is discarded
from cache. Cancellation, origin body error or trailers discard pending fills.
Corrupt stored metadata evicts the matching entry before ordinary origin fallback.
After response headers commit, transport errors follow the selected response;
there is no second origin request.

Publication follows a successful native response finish: HTTP/1 proves local
flush and HTTP/2 proves accepted EOS, neither proves receipt by the remote peer.
Empty cache statistics can retain process infrastructure bytes. Leak checks use
allocated minus infrastructure together with pending bytes, entries and reader pins.

## Related links

- [Plugin overview](README.md)
- [GraphQL guard](graphql-guard.md)
- [Key authentication](key-auth.md)
- [HTTP caching semantics](https://www.rfc-editor.org/rfc/rfc9111)
