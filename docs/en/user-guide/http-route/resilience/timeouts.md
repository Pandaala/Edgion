# Timeout Configuration

Configure request timeouts to improve system resilience.

## Configuration

```yaml
rules:
  - matches:
      - path:
          type: PathPrefix
          value: /api
    timeouts:
      request: 30s
      backendRequest: 10s
    backendRefs:
      - name: api-service
        port: 8080
```

## Configuration Reference

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| request | duration | | | Total request timeout (including retries) |
| backendRequest | duration | | | Single backend request timeout |

## Timeout Types

### request - Request Timeout

The total time from receiving the request to returning the response, including:
- All retry attempts
- Waiting for backend responses

### backendRequest - Backend Request Timeout

The timeout for a single backend request, excluding retries.

## Examples

### Example 1: Basic Timeout Configuration

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: timeout-example
spec:
  parentRefs:
    - name: my-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api
      timeouts:
        request: 60s
        backendRequest: 15s
      backendRefs:
        - name: api-service
          port: 8080
```

### Example 2: Long-Lived Connection Scenario

```yaml
rules:
  - matches:
      - path:
          type: PathPrefix
          value: /stream
    timeouts:
      request: 3600s  # 1 hour
    backendRefs:
      - name: streaming-service
        port: 8080
```

## Related Documentation

- [Retry Policy](./retry.md)
- [Session Persistence](./session-persistence.md)
