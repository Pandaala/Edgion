# Session Persistence

Configure session persistence (Session Affinity) to ensure requests from the same client are routed to the same backend.

## Configuration

```yaml
rules:
  - sessionPersistence:
      sessionName: my-session
      type: Cookie
      cookieConfig:
        lifetimeType: Permanent
        path: /
    backendRefs:
      - name: app-service
        port: 8080
```

## Configuration Reference

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| sessionName | string | | | Session name |
| type | string | | Cookie | Session type |
| absoluteTimeout | duration | | | Absolute timeout |
| idleTimeout | duration | | | Idle timeout |
| cookieConfig | object | | | Cookie configuration |

### cookieConfig

| Field | Type | Description |
|-------|------|-------------|
| lifetimeType | string | Session or Permanent |
| path | string | Cookie path |

## Session Types

### Cookie

Cookie-based session persistence:

```yaml
sessionPersistence:
  type: Cookie
  sessionName: SERVERID
  cookieConfig:
    lifetimeType: Permanent
```

### Header

Header-based session persistence:

```yaml
sessionPersistence:
  type: Header
  sessionName: X-Session-ID
```

## Examples

### Example 1: Cookie Session Persistence

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: session-sticky
spec:
  parentRefs:
    - name: my-gateway
  rules:
    - sessionPersistence:
        type: Cookie
        sessionName: BACKEND_ID
        cookieConfig:
          lifetimeType: Session
      backendRefs:
        - name: stateful-app
          port: 8080
          weight: 50
        - name: stateful-app-2
          port: 8080
          weight: 50
```

## Related Documentation

- [Timeout Configuration](./timeouts.md)
- [Retry Policy](./retry.md)
