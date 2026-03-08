# Retry Policy

Configure request retries to improve availability.

## Configuration

```yaml
rules:
  - matches:
      - path:
          type: PathPrefix
          value: /api
    retry:
      attempts: 3
      backoff:
        baseInterval: 100ms
        maxInterval: 10s
      retryOn:
        - "5xx"
        - "reset"
        - "connect-failure"
    backendRefs:
      - name: api-service
        port: 8080
```

## Configuration Reference

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| attempts | int | | 1 | Maximum number of retries |
| backoff.baseInterval | duration | | 25ms | Base retry interval |
| backoff.maxInterval | duration | | 250ms | Maximum retry interval |
| retryOn | []string | | | Conditions that trigger retries |

## Retry Conditions

| Condition | Description |
|-----------|-------------|
| 5xx | HTTP 5xx responses |
| reset | Connection reset |
| connect-failure | Connection failure |
| retriable-4xx | Retriable 4xx (408, 429) |
| refused-stream | Stream refused |
| cancelled | Request cancelled |
| deadline-exceeded | Timeout |
| unavailable | Service unavailable |

## Examples

### Example 1: Basic Retry

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: retry-example
spec:
  parentRefs:
    - name: my-gateway
  rules:
    - retry:
        attempts: 3
        retryOn:
          - "5xx"
          - "connect-failure"
      backendRefs:
        - name: api-service
          port: 8080
```

### Example 2: Exponential Backoff

```yaml
retry:
  attempts: 5
  backoff:
    baseInterval: 100ms
    maxInterval: 10s
  retryOn:
    - "5xx"
```

Retry intervals: 100ms -> 200ms -> 400ms -> 800ms -> 1600ms (max 10s)

## Notes

- Only enable retries for idempotent operations
- Set reasonable timeouts in combination with retries
- Monitor retry rates; excessively high rates may indicate backend issues

## Related Documentation

- [Timeout Configuration](./timeouts.md)
- [Session Persistence](./session-persistence.md)
