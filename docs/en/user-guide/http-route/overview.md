# HTTPRoute Overview

HTTPRoute is the core Gateway API resource for routing HTTP/HTTPS traffic, mapping incoming requests to backend services based on match conditions, applying filters, and splitting traffic across backends.

## Basic structure

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: example-route
  namespace: production
spec:
  # Which Gateway listener(s) to attach to
  parentRefs:
    - name: main-gateway
      sectionName: http
  # Host header values this route matches
  hostnames:
    - "app.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api
      backendRefs:
        - name: api-service
          port: 8080
```

Key top-level fields:

| Field | Description |
|-------|-------------|
| `parentRefs` | Attaches the route to one or more Gateway listeners |
| `hostnames` | Matches the request Host header (exact or `*.` wildcard) |
| `rules` | List of routing rules, each with matches, filters, and backendRefs |

## Core concepts

### Hostnames

`hostnames` accepts up to 16 lowercase DNS names, including short names such as `api`. A wildcard such as `*.example.com` matches both `api.example.com` and `a.b.example.com`, but not `example.com`. IP literals, malformed names and a bare `*` are invalid; one invalid entry rejects the whole list. An omitted or empty list means no Route hostname restriction, not exemption from Listener checks.

Matching declared names are considered from exact to longest wildcard suffix, before hostless routes. Listener intersection limits eligibility without promoting a wildcard route's priority. If path or other match conditions fail, another eligible candidate may match; a more-specific Listener is not bypassed merely because it has no matching route. These routing wildcards do not change certificate matching rules.

### ParentRefs

`parentRefs` attaches the route to a Gateway listener. The Controller intersects the route's hostnames with the listener's hostnames and binds the route only to matching listeners.

You can apply an HTTPRoute and its parent Gateway in either order. If the route arrives first, the Controller reconciles it again when the Gateway becomes available.

In FileSystem mode, set `metadata.namespace` explicitly and use the matching filename, such as `default/HTTPRoute_default_my-route.yaml`. A missing namespace produces a `ParseError` in the file's status.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | **Required** | Gateway name |
| `namespace` | String | none | Gateway namespace; cross-namespace requires a `ReferenceGrant` |
| `sectionName` | String | none | Specific listener name on the Gateway |
| `port` | Integer | none | Specific listener port |

A single HTTPRoute can reference multiple Gateways to share routing rules. See [Backend configuration](./backends/README.md) for cross-namespace details.

When both `sectionName` and `port` are set, the listener must match both. Sharing a physical
port with another Gateway does not expand a parentRef's attachment: a reference to Gateway A
on port 8080 does not authorize Gateway A on port 8081 through another parentRef.

### Rules

Each entry in `rules` is an independent routing decision unit containing match conditions, filters, and backend references. When a request matches multiple rules, the most specific match wins (longer path prefixes and exact paths take precedence over shorter prefixes).

### Matches

`matches` defines the conditions a request must satisfy. You can match on path, headers, query parameters, and HTTP method. Conditions inside a single match are ANDed; multiple matches in one rule are ORed. See [Match rules](./matches/README.md).

### Filters / Plugins

`filters` run processing logic before the request reaches the backend or before the response returns to the client:

- **Gateway API filters handled by Edgion**: `RequestHeaderModifier`, `ResponseHeaderModifier`, `RequestRedirect`, `URLRewrite`, `RequestMirror`, and native `CORS`. The upstream `ExternalAuth` `backendRef`/`protocol` form is rejected; use Edgion's separate ForwardAuth plugin for that behavior.
- **Edgion extension plugins**: referenced through `ExtensionRef` pointing at an `EdgionPlugins` resource (rate limiting, authentication, DSL, and more).

`URLRewrite` and `RequestRedirect` are mutually exclusive within the same rule. See [Filters](./filters/README.md).

### BackendRefs

`backendRefs` lists the backend services that receive matched traffic:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | **Required** | Service name |
| `port` | Integer | none | Target port |
| `weight` | Integer | `1` | Traffic weight, interpreted as a ratio for weighted round-robin |
| `namespace` | String | none | Service namespace; cross-namespace requires a `ReferenceGrant` |

Multiple weighted backends enable canary releases and blue-green deployments. See [Backend configuration](./backends/README.md) and [Load balancing algorithms](./lb-algorithms.md).

## Complete example

This HTTPRoute covers path matching, header modification, traffic splitting, and a redirect across several rules:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: app-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: http
  hostnames:
    - "app.example.com"
  rules:
    # Rule 1: API v2 routing with a custom header
    - matches:
        - path:
            type: PathPrefix
            value: /api/v2
      filters:
        - type: RequestHeaderModifier
          requestHeaderModifier:
            add:
              - name: X-API-Version
                value: "v2"
      backendRefs:
        - name: api-v2-service
          port: 8080

    # Rule 2: Canary release — 90% stable, 10% canary
    - matches:
        - path:
            type: PathPrefix
            value: /api/v1
      backendRefs:
        - name: api-v1-stable
          port: 8080
          weight: 90
        - name: api-v1-canary
          port: 8080
          weight: 10

    # Rule 3: Redirect a retired path
    - matches:
        - path:
            type: Exact
            value: /old-page
      filters:
        - type: RequestRedirect
          requestRedirect:
            path:
              type: ReplaceFullPath
              replaceFullPath: /new-page
            statusCode: 301

    # Rule 4: Default backend with resilience policies
    - timeouts:
        request: "30s"
        backendRequest: "10s"
      retry:
        attempts: 3
        backoff: "1s"
        codes:
          - 502
          - 503
          - 504
      backendRefs:
        - name: frontend-service
          port: 8080
```

## Subdocument navigation

| Topic | Document | Description |
|-------|----------|-------------|
| Match rules | [matches/](./matches/README.md) | Path, header, query parameter, and method matching |
| Filters | [filters/](./filters/README.md) | Gateway API standard filters and Edgion extension plugins |
| Backend configuration | [backends/](./backends/README.md) | Weighted traffic splitting, cross-namespace references, health checks |
| Load balancing | [lb-algorithms.md](./lb-algorithms.md) | RoundRobin, ConsistentHash, LeastConnection, and EWMA algorithms |
| Resilience | [resilience/](./resilience/README.md) | Timeouts, retries, and session persistence |
