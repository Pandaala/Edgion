---
name: httproute-features
description: Full HTTPRoute schema and features: match conditions, filters, backend references, timeouts/retries, session persistence.
---

# HTTPRoute Resource

> API: `gateway.networking.k8s.io/v1` | Scope: Namespaced
> Gateway API v1.4.0 Core resource

HTTPRoute is the most complex routing resource in Edgion, defining routing rules at the HTTP/HTTPS layer.

## Full Schema

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: my-route
  namespace: default
spec:
  parentRefs:                                      # Attach to a Gateway (see 00-common-concepts.md)
    - name: my-gateway
      sectionName: https

  hostnames:                                       # List of matched hostnames
    - "api.example.com"
    - "*.example.com"                              # Wildcards supported

  rules:
    - name: "api-v1"                               # Rule name (optional, used for observability)

      matches:                                     # Match conditions (OR relationship)
        - path:
            type: PathPrefix                       # Exact | PathPrefix | RegularExpression
            value: "/api/v1"
          headers:
            - type: Exact                          # Exact | RegularExpression
              name: X-Version
              value: "v1"
          queryParams:
            - type: Exact                          # Exact | RegularExpression
              name: debug
              value: "true"
          method: GET                              # HTTP method

      filters:                                     # Filter chain
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set:
              - name: X-Gateway
                value: edgion
            add:
              - name: X-Request-ID
                value: "{{generated}}"
            remove:
              - X-Internal

        - type: ResponseHeaderModifier
          responseHeaderModifier:
            set:
              - name: X-Powered-By
                value: Edgion

        - type: RequestRedirect
          requestRedirect:
            scheme: https
            hostname: new.example.com
            port: 443
            statusCode: 301                        # 301 | 302
            path:
              type: ReplaceFullPath                # ReplaceFullPath | ReplacePrefixMatch
              replaceFullPath: /new-path

        - type: URLRewrite
          urlRewrite:
            hostname: internal.example.com
            path:
              type: ReplacePrefixMatch
              replacePrefixMatch: /v2

        - type: RequestMirror
          requestMirror:
            backendRef:
              name: mirror-service
              port: 8080
            fraction:
              numerator: 10
              denominator: 100                     # Mirror 10%
            connectTimeoutMs: 1000                 # Connect timeout (ms)
            writeTimeoutMs: 1000                   # Write timeout (ms)
            maxBufferedChunks: 5                   # Max buffered body chunks
            mirrorLog: true                        # Whether mirror keeps a separate access log
            maxConcurrent: 1024                    # Max concurrent mirror tasks
            channelFullTimeoutMs: 0                # Wait when channel is full (ms)

        - type: ExtensionRef                       # Reference an EdgionPlugins resource
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: my-plugins

      backendRefs:                                 # Backend references
        - name: api-service
          port: 8080
          weight: 80
        - name: api-service-canary
          port: 8080
          weight: 20
          filters:                                 # backendRef-level filters
            - type: RequestHeaderModifier
              requestHeaderModifier:
                set:
                  - name: X-Canary
                    value: "true"

      timeouts:                                    # Gateway API v1.4 standard timeouts
        request: "30s"                             # End-to-end request timeout (including retries)
        backendRequest: "10s"                      # Per-backend request timeout

      retry:                                       # Gateway API v1.4 standard retry
        attempts: 3                                # Maximum retry attempts
        backoff: "1s"                              # Retry interval
        codes:                                     # HTTP status codes that trigger a retry
          - 502
          - 503
          - 504

      sessionPersistence:                          # Session persistence
        sessionName: "EDGION_SESSION"              # Session token name
        absoluteTimeout: "1h"                      # Absolute timeout
        idleTimeout: "30m"                         # Idle timeout
        type: Cookie                               # Cookie | Header
        cookieConfig:
          lifetimeType: Permanent                  # Permanent | Session
```

## Match Condition Schema

### HTTPRouteMatch

Multiple matches have an **OR** relationship; the path/headers/queryParams/method inside a single match have an **AND** relationship.

#### path

| Field | Type | Description |
|-------|------|-------------|
| `type` | `String` | `Exact` / `PathPrefix` / `RegularExpression` |
| `value` | `String` | Match value (PathPrefix defaults to `/`); for `Exact` / `PathPrefix` may contain `:name` path-parameter segments — see below |

##### Path parameters (`:name` syntax)

For `Exact` and `PathPrefix` matches, `value` may include **named path parameters**:

| Pattern | Behavior |
|---------|----------|
| `/users/:id` | `id` segment is a wildcard; the actual value is captured |
| `/api/:ver/users/:uid` | Multiple captures in one route |
| `/data/::literal` | `::` escapes to a literal `:`; the segment is `:literal`, not a parameter |

Where the captured values are usable at runtime:

| Consumer | How to read |
|----------|-------------|
| **DSL plugin** (`type: Dsl`) | Builtin `req.path_param("name")` — returns `String?` (nil when absent) |
| **ProxyRewrite plugin** | `$name` substitution inside `uri` / `headers.set[*].value` (alongside `$uri`, `$arg_xxx`, `$1-$9`) |

Not supported by path-parameter substitution (deliberately, to stay aligned with Gateway API semantics):

- `URLRewrite`, `RequestRedirect`, `RequestHeaderModifier`, `ResponseHeaderModifier` — these treat their value strings as literals; use `ProxyRewrite` if you need template variables.
- `RegularExpression` path matches — use regex capture groups (`$1`-`$9` in `ProxyRewrite`) instead.

Example (HTTPRoute + ProxyRewrite):

```yaml
- matches:
    - path:
        type: PathPrefix
        value: /params/:uid/info
  filters:
    - type: ExtensionRef
      extensionRef: { group: edgion.io, kind: EdgionPlugins, name: rewrite-with-uid }
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata: { name: rewrite-with-uid, namespace: edgion-default }
spec:
  requestPlugins:
    - type: ProxyRewrite
      config:
        uri: "/user-service/$uid/profile"
        headers:
          set:
            - { name: X-User-Id, value: "$uid" }
```

#### headers

| Field | Type | Description |
|-------|------|-------------|
| `type` | `String` | `Exact` / `RegularExpression` |
| `name` | `String` | Header name (case-insensitive) |
| `value` | `String` | Match value |

#### queryParams

| Field | Type | Description |
|-------|------|-------------|
| `type` | `String` | `Exact` / `RegularExpression` |
| `name` | `String` | Query parameter name |
| `value` | `String` | Match value |

#### method

`String`: `GET` / `POST` / `PUT` / `DELETE` / `PATCH` / `HEAD` / `OPTIONS` / `CONNECT`

## Filter Types

| Type | Description | Stage |
|------|-------------|-------|
| `RequestHeaderModifier` | Modify request headers (set/add/remove) | Request stage |
| `ResponseHeaderModifier` | Modify response headers (set/add/remove) | Response stage |
| `RequestRedirect` | HTTP redirect (301/302) | Request stage (terminating) |
| `URLRewrite` | URL rewrite (hostname/path) | Request stage |
| `RequestMirror` | Mirror requests to a backup backend | Request stage (non-blocking) |
| `ExtensionRef` | Reference an EdgionPlugins extension plugin | Depends on the plugin |

### Filter Mutual-Exclusion Constraints

`URLRewrite` and `RequestRedirect` are **mutually exclusive** and cannot appear in the same rule. The conflict check covers three levels:

1. **Rule level**: `rule.filters` contains both URLRewrite and RequestRedirect.
2. **BackendRef level**: `backendRef.filters` contains both.
3. **Cross-level**: `rule.filters` contains one and `backendRef.filters` contains the other.

A rule that violates the constraint is marked `PartiallyInvalid`; that rule does not take effect, but other valid rules in the HTTPRoute continue to work.

### ExtensionRef Reuse Convention

`ExtensionRef` is the standard Gateway API filter, referencing one `EdgionPlugins` resource by `namespace/name`. **The `type: ExtensionRef` (the "plugin referencing plugin" nested form) inside `EdgionPlugins.spec.\*Plugins` was removed in release `<version>`.** Multi-plugin reuse is expressed in two ways:

- **Aggregate mode**: combine multiple plugins into one `EdgionPlugins` resource; the route only needs a single `ExtensionRef`.
- **Side-by-side mode**: write multiple `ExtensionRef` entries side-by-side in `rules[].filters`, each pointing to a different `EdgionPlugins`.

See `docs/zh-CN/user-guide/http-route/filters/plugin-composition.md` and `docs/zh-CN/migrations/<version>-extension-ref.md` for details.

## Timeouts and Retries

### timeouts (Gateway API v1.4)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `request` | `Duration` | EdgionGatewayConfig global value | End-to-end request timeout (**including retries**) |
| `backendRequest` | `Duration` | EdgionGatewayConfig global value | Per-backend request timeout (must not exceed `request`) |

> **Constraint**: `backendRequest` must not exceed `request`. If the configured `backendRequest` is greater than `request`, it is automatically clamped to `request`, ensuring that a single backend request timeout does not exceed the end-to-end limit.

### retry (Gateway API v1.4)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `attempts` | `i32` | `EdgionGatewayConfig.spec.maxRetries` (default 3) | Max retry attempts (`0` disables retry) |
| `backoff` | `Duration` | — | Minimum retry interval (exponential backoff base, capped at 30s) |
| `codes` | `Vec<i32>` | — | HTTP status codes that trigger a retry (100-599) |

> **Priority**: `rule.retry.attempts` > `EdgionGatewayConfig.spec.maxRetries` (global default). Connection failures and status-code retries share the same attempts counter. The `backendRequest` timeout simultaneously serves as the route-level override for Pingora `connection_timeout` / `read_timeout` / `write_timeout`.

## Session Persistence Schema

```yaml
sessionPersistence:
  sessionName: String?               # Session token name (default edgion-session-affinity)
  absoluteTimeout: Duration?         # Absolute timeout (mapped to Max-Age for Permanent Cookies)
  idleTimeout: Duration?             # Idle timeout (not supported in the current version, reserved for Redis mode)
  type: Cookie | Header              # Persistence type (default Cookie)
  cookieConfig:
    lifetimeType: Permanent | Session # Cookie lifetime
  strict: bool?                      # Edgion extension: strict mode (default false)
```

### Runtime Behavior

- **Token generation**: CRC32 hash of the backend's `ip:port` (8-character hex), does not expose the backend topology.
- **Cookie mode**: the first response injects `Set-Cookie: <name>=<token>; Path=/; HttpOnly; SameSite=Lax` (Permanent adds `Max-Age`).
- **Header mode**: the first response injects `<name>: <token>`; the client must include it in subsequent requests.
- **Backend selection**: the token is used as a consistent-hashing key, ensuring the same token is routed to the same backend.
- **Fallback** (default): when the bound backend is unavailable, fall back to normal load balancing and update the token.
- **Strict mode**: when `strict: true`, do not fall back; return 503.

### Priority with Plugins

DirectEndpoint / ExternalJump > Session Persistence > InternalJump > Round-Robin

## Traffic Splitting

Achieved via the `weight` field in `backendRefs`:

```yaml
backendRefs:
  - name: v1-service
    port: 8080
    weight: 90    # 90% of traffic
  - name: v2-service
    port: 8080
    weight: 10    # 10% of traffic (canary)
```

## Edgion Extension Annotations

The following annotations are placed in `HTTPRoute.metadata.annotations` (same for GRPCRoute). For the full specification and parsing details, see the "HTTPRoute / GRPCRoute" section of [`skills/02-features/05-annotations/references/annotations-metadata.md`](../05-annotations/references/annotations-metadata.md).

| Annotation | Type | Default | Applicable Resource | Description |
|------------|------|---------|---------------------|-------------|
| `edgion.io/forward-raw-path` | `"true"` \| `"false"` | absent (use the global switch) | HTTPRoute | Override path normalization behavior. `"true"` forces forwarding of the raw path; `"false"` forces normalization; absent uses the global `forward_normalized_path`. Invalid values produce an INFO-level warning and the absent-value behavior. |
| `edgion.io/hostname-resolution` | controller-written string | system-managed | HTTPRoute, GRPCRoute | A diagnostic annotation written by the controller during resolution that describes the hostname resolution outcome. **Do not write this field manually.** |
