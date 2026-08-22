---
name: httproute-features
description: Full HTTPRoute schema and features: match conditions, filters, backend references, timeouts/retries, session persistence.
---

# HTTPRoute Resource

> API: `gateway.networking.k8s.io/v1` | Scope: Namespaced
> Gateway API v1.5.0 Core resource

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
          requestMirror:                           # Standard fields only: backendRef / fraction / percent.
            backendRef:                            # Tuning knobs use edgion.io/mirror-* route annotations
              name: mirror-service                 # (the vendored Gateway API CRD prunes non-standard
              port: 8080                           # filter fields in K8s mode).
            fraction:
              numerator: 10
              denominator: 100                     # Mirror 10%

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

### MCP direct-route subset

When the sole backendRef resolves to the current `spec.mcp` variant of an EdgionBackend, the rule
uses a deliberately narrower subset than general HTTP routing:

- one positive-weight `group: edgion.io`, `kind: EdgionBackend` backendRef with no port;
- one explicit `Exact` path match and no method, header, or query predicates;
- `/` or slash-separated non-empty ASCII segments containing only letters, digits, `-`, `.`, `_`,
  or `~`; `.` and `..` segments are invalid;
- no path parameters or escaped literal colon syntax, percent escapes, repeated/trailing slash,
  query, fragment, session persistence, or delegation.

The Controller derives this classification from the current backend generation and publishes no
MCP marker. Invalid rules use the normal `invalidRuleIndices` and HTTPRoute status model. This
eligibility classification does not activate an MCP listener or wire protocol; Gateway recognition,
Origin validation, transport, and public activation are later WP10 stages.

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

See `docs/en/user-guide/http-route/filters/plugin-composition.md` and
`docs/en/migrations/<version>-extension-ref.md` for details; paired Chinese
pages live under `docs/zh-CN/`.

## Timeouts and Retries

### timeouts (Gateway API v1.4)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `request` | `Duration` | EdgionGatewayConfig global value | End-to-end request timeout (**including retries**) |
| `backendRequest` | `Duration` | EdgionGatewayConfig global value | Per-backend request timeout (must not exceed `request`). When the rule has a retry policy, acts as the GEP-1731 per-try timeout: an attempt exceeding it is canceled and retried |

> **Constraint**: `backendRequest` must not exceed `request`. If the configured `backendRequest` is greater than `request`, it is automatically clamped to `request`, ensuring that a single backend request timeout does not exceed the end-to-end limit.

### retry (Gateway API v1.4)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `attempts` | `i32` | `EdgionGatewayConfig.spec.maxRetries` (default 3) | Max retry attempts (`0` disables retry) |
| `backoff` | `Duration` | — | Minimum retry interval (exponential backoff base, capped at 30s) |
| `codes` | `Vec<i32>` | — | HTTP status codes that trigger a retry (100-599) |

> **Priority**: `rule.retry.attempts` > `EdgionGatewayConfig.spec.maxRetries` (global default). Connection failures and status-code retries share the same attempts counter. The `backendRequest` timeout simultaneously serves as the route-level override for Pingora `connection_timeout` / `read_timeout` / `write_timeout`.

> **Per-try timeout**: with a retry policy present, `timeouts.backendRequest`
> bounds each attempt and a timed-out attempt is retried while nothing has
> been sent downstream yet (recorded as
> `EdgionStatus::BackendRequestTimeoutRetry`, 504_03, in the access-log error
> chain). Deadline-sourced timeouts (`request` / client `grpc-timeout`) are
> never retried. Implemented in `configure_peer_timeouts`
> (`pg_upstream_peer.rs`, `fold_peer_timeouts`) + `error_while_proxy` Case
> 1.5. Integration scenario: `HTTPRoute/Backend/PerTryTimeout`.

## Session Persistence Schema

```yaml
sessionPersistence:
  sessionName: String?               # Session token name (default edgion-session-affinity)
  absoluteTimeout: Duration?         # Gateway-enforced lifetime (GEP-2257), embedded in the token; also Max-Age for Permanent Cookies
  idleTimeout: Duration?             # Idle timeout (not supported in the current version, reserved for Redis mode)
  type: Cookie | Header              # Persistence type (default Cookie)
  cookieConfig:
    lifetimeType: Permanent | Session # Cookie lifetime
  strict: bool?                      # Edgion extension: strict mode (default false)
```

### Runtime Behavior

- **Token generation**: truncated SHA-256 hash of the backend's `ip:port` (16-character hex, 64-bit identity), does not expose the backend topology. When `absoluteTimeout` is set, the token is `<hash>.<expiry_epoch>` (absolute expiry frozen at issuance).
- **Absolute lifetime**: `absoluteTimeout` (GEP-2257 `h|m|s|ms`, 1–4 segments; ceil to seconds, min 1s) is **gateway-enforced via the embedded expiry** in BOTH Cookie and Header modes and for BOTH `Session` and `Permanent` cookies. A replayed token past its expiry is treated like a missing token: normal LB selects a backend and a fresh token is issued. `Permanent` requires `absoluteTimeout`; an invalid `sessionPersistence` block drops that rule (`PartiallyInvalid` in K8s; dropped with an ERROR log in FileSystem/Etcd), never a silent never-expiring token.
- **Cookie mode**: the first response injects `Set-Cookie: <name>=<token>; Path=/; HttpOnly; SameSite=Lax` (Permanent adds `Max-Age`, matching the embedded expiry).
- **Header mode**: the first response injects `<name>: <token>`; the client must include it in subsequent requests.
- **Backend selection**: the token is resolved to its exact backend BEFORE weighted backendRef selection, across ALL of the matched rule's backendRefs in declaration order (GEP-1619: weights govern only sessions without a token; a bound backendRef whose weight drops to 0 keeps its established sessions). Resolution is scoped to the matched rule, so default-named sessions on rules with disjoint backends never leak across rules.
- **Fallback** (default): when the bound backend is unavailable, fall back to normal (weighted) load balancing and update the token.
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

## Route Delegation (Multi-Tenancy)

A rule delegates its path prefix to child HTTPRoutes (possibly in other
namespaces) by pointing its **single** `backendRef` at
`group: gateway.networking.k8s.io, kind: HTTPRoute` (`name: "*"` = every route
in the target namespace). The controller flattens the tree at parse time into
the controller-resolved `resolved_rules` field; the data plane never sees the
nesting.

```yaml
rules:
  - matches:
      - path: { type: PathPrefix, value: /api/v1/billing }   # PathPrefix only
    filters:              # mandatory, inherited by all child rules
      - type: RequestHeaderModifier
        requestHeaderModifier: { set: [{ name: x-tenant, value: billing }] }
    backendRefs:
      - group: gateway.networking.k8s.io
        kind: HTTPRoute
        name: "*"
        namespace: billing
```

Key semantics (canonical user doc: [`docs/en/user-guide/http-route/delegation.md`](../../../docs/en/user-guide/http-route/delegation.md); architecture: [`../../01-architecture/05-resources/04-http-route.md`](../../01-architecture/05-resources/04-http-route.md)):

- Child paths are absolute and must fall inside the delegated prefix (segment
  boundary); out-of-prefix child rules are dropped + reported (`PartiallyInvalid`).
- Children must not set `hostnames`; children may delegate further (cycle
  detection, depth cap 8); children may reverse-lock parents via
  `parentRefs {kind: HTTPRoute}`.
- Parent standard filters are mandatory (same-type child filter ⇒ rule
  dropped); ExtensionRef chains concatenate (parent first). `timeouts` /
  `retry` / `sessionPersistence`: child-first-else-inherit.
- Parent and child ExtensionRefs resolve EdgionPlugins in the Namespace of the
  HTTPRoute that declares each filter; flattening never changes that locality.
- Child backendRef namespace defaults to the child's namespace; cross-ns
  Service refs are ReferenceGrant-checked with the child as referrer.
- Additive: `backendRef {kind: HTTPRoute}` was previously invalid config;
  `allowedRoutes` semantics are untouched.

Internal processing: [`01-architecture/05-resources/04-http-route.md`](../../01-architecture/05-resources/04-http-route.md).

## Edgion Extension Annotations

The following annotations are placed in `HTTPRoute.metadata.annotations` (same for GRPCRoute). For the full specification and parsing details, see the "HTTPRoute / GRPCRoute" section of [`skills/02-features/05-annotations/references/annotations-metadata.md`](../05-annotations/references/annotations-metadata.md).

| Annotation | Type | Default | Applicable Resource | Description |
|------------|------|---------|---------------------|-------------|
| `edgion.io/forward-raw-path` | `"true"` \| `"false"` | absent (use the global switch) | HTTPRoute | Override path normalization behavior. `"true"` forces forwarding of the raw path; `"false"` forces normalization; absent uses the global `forward_normalized_path`. Invalid values produce an INFO-level warning and the absent-value behavior. |
| `edgion.io/hostname-resolution` | controller-written string | system-managed | HTTPRoute, GRPCRoute | A diagnostic annotation written by the controller during resolution that describes the hostname resolution outcome. **Do not write this field manually.** |
