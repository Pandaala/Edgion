# Session Persistence

Configure session persistence (Session Affinity) to ensure requests from the same client are routed to the same backend.

## How It Works

1. **First request**: Client has no session token → gateway selects a backend via normal (weighted) load balancing → injects session token in response (Cookie or Header)
2. **Subsequent requests**: Client carries session token → the gateway resolves it against the rule's backends **before** weighted selection and routes to the exact backend that issued it
3. **Backend unavailable**: By default, falls back to another healthy backend and updates the session token; in strict mode, returns an error

The session token is a truncated SHA-256 hash of the backend address (16 lowercase hex characters, e.g., `a1b2c3d4e5f60718`), not exposing backend topology.

## Traffic Splitting (Weighted BackendRefs)

Per Gateway API (GEP-1619), **weights govern only new sessions**. Once a session
is established, it keeps routing to its original Service and endpoint across
ALL of the rule's backendRefs:

- The token is resolved across every backendRef of the matched rule (in
  declaration order) before weighted selection, so a 70/30 split cannot bounce
  an established session between Services.
- Changing the bound backendRef's weight — **including to 0** — does not move
  an established session while its endpoint is still serving.
- Deleting the bound endpoint (or the backendRef) triggers the normal fallback:
  the request is served by weighted selection and the response carries a
  refreshed token.
- Session resolution is scoped to the matched route rule: two rules using the
  default session name with disjoint backends never share sessions. Rules that
  reference the same Service resolve to the same pod (matching Envoy Gateway).

```yaml
rules:
  - sessionPersistence:
      type: Cookie
      sessionName: canary-session
      absoluteTimeout: "1h"
      cookieConfig:
        lifetimeType: Permanent
    backendRefs:
      - name: app-v1
        port: 8080
        weight: 70        # new sessions: ~70%
      - name: app-v2
        port: 8080
        weight: 30        # new sessions: ~30%; established v2 sessions stay on v2
```

## Configuration

```yaml
rules:
  - sessionPersistence:
      sessionName: my-session              # Cookie/Header name
      type: Cookie                         # Cookie | Header
      absoluteTimeout: "1h"                # Absolute timeout (maps to Cookie Max-Age)
      cookieConfig:
        lifetimeType: Permanent            # Permanent | Session
    backendRefs:
      - name: app-service
        port: 8080
```

## Configuration Reference

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| sessionName | string | No | `edgion-session-affinity` | Session token name (Cookie name or Header name) |
| type | string | No | Cookie | Session type: `Cookie` or `Header` |
| absoluteTimeout | duration | No | — | Absolute timeout (maps to Cookie `Max-Age` in Permanent mode) |
| idleTimeout | duration | No | — | Idle timeout (not supported in current version, reserved for Redis mode) |
| cookieConfig | object | No | — | Cookie-specific configuration |
| strict | bool | No | false | Edgion extension: strict mode (see below) |

### cookieConfig

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| lifetimeType | string | Session | `Session` (expires on browser close) or `Permanent` (requires absoluteTimeout for Max-Age) |

## Session Types

### Cookie (default)

Cookie-based session persistence. The gateway injects `Set-Cookie` in the first response; the client automatically carries it in subsequent requests.

```yaml
sessionPersistence:
  type: Cookie
  sessionName: BACKEND_ID
  absoluteTimeout: "24h"
  cookieConfig:
    lifetimeType: Permanent
```

Cookie attributes: `Path=/; Max-Age=86400; HttpOnly; SameSite=Lax`. The `Secure` attribute is added automatically when the request arrives over TLS. `Max-Age` is emitted only for `lifetimeType: Permanent` with an `absoluteTimeout`.

### Header

Header-based session persistence. The gateway adds a response header on the first request; the client must include it in subsequent requests.

```yaml
sessionPersistence:
  type: Header
  sessionName: X-Session-ID
```

## Fallback and Strict Mode

### Default Behavior (Fallback)

When the session-bound backend is unavailable (health check failure, backend down, etc.), the gateway automatically falls back to another healthy backend and updates the session token. Subsequent requests will route to the new backend.

This matches the default behavior of Nginx, Envoy, and HAProxy.

### Strict Mode

With `strict: true`, the gateway returns an error (503) when the session-bound backend is unavailable, without fallback. Use this for scenarios requiring strict session consistency.

```yaml
sessionPersistence:
  type: Cookie
  sessionName: BACKEND_ID
  strict: true
  cookieConfig:
    lifetimeType: Permanent
```

> Similar to Envoy's `strict` field on the stateful session filter and HAProxy's `force-persist`.

## Priority with Plugins

| Plugin | Priority | Relationship with Session Persistence |
|--------|----------|--------------------------------------|
| DirectEndpoint | Highest | Specifies exact backend IP:port, bypasses session persistence |
| DynamicExternalUpstream | High | External domain routing, bypasses session persistence |
| DynamicInternalUpstream | High | Explicit per-request BackendRef override: wins over the sticky session for that request; the session binding is left untouched (no token rewrite), and the next non-overridden request resolves normally |
| **Session Persistence** | **Medium** | **Exact backend affinity based on cookie/header token** |
| Weighted Round-Robin | Lowest | Governs sessions without a token (initial distribution) |

## Examples

### Example 1: Cookie Session Persistence (with Fallback)

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: session-sticky
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - sessionPersistence:
        type: Cookie
        sessionName: BACKEND_ID
        absoluteTimeout: "1h"
        cookieConfig:
          lifetimeType: Permanent
      backendRefs:
        - name: stateful-app
          port: 8080
```

### Example 2: Header Session Persistence (Strict Mode)

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: session-strict
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - sessionPersistence:
        type: Header
        sessionName: X-Session-ID
        strict: true
      backendRefs:
        - name: stateful-app
          port: 8080
```

## Limitations

- **Multi-instance consistency**: The token derivation is deterministic (an unkeyed hash of the backend address), so as long as backend lists are identical across Gateway instances, the same token routes to the same backend with no server-side session storage. Removing the bound endpoint invalidates its sessions (fallback + token refresh); other endpoints' sessions are unaffected.
- **idleTimeout**: Not supported in the current version (pure client-side Cookie approach cannot implement server-side idle tracking), reserved for future Redis mode.
- **GRPCRoute**: Natively supported with the same configuration surface and verified by backend-identity integration tests. gRPC-specific semantics: with `strict: true`, an unavailable pinned backend fails closed with `grpc-status: 14` (UNAVAILABLE) rather than HTTP 503. Native gRPC clients (tonic, grpc-go) must echo the session cookie at the application layer; browser grpc-web clients send it automatically only for same-site requests with credentials enabled (the cookie carries `SameSite=Lax`), so cross-site grpc-web deployments should prefer `type: Header`. Session persistence requires EndpointSlice-backed Services (ClusterIP-kind backends bypass it, same as HTTPRoute).

## Related Documentation

- [Timeout Configuration](./timeouts.md)
- [Retry Policy](./retry.md)
- [Load Balancing Algorithms](../lb-algorithms.md)
