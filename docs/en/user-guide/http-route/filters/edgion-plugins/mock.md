# Mock Plugin

> **🔌 Edgion Extension**
> 
> Mock is a mock response plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

## Overview

The Mock plugin returns preset HTTP responses without forwarding requests to upstream services. Useful for API prototyping, interface testing, health check endpoints, and error simulation.

## Quick Start

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: mock-plugin
spec:
  requestPlugins:
    - enable: true
      type: Mock
      config:
        statusCode: 200
        body: '{"status": "ok", "message": "Service is healthy"}'
        contentType: "application/json"
```

---

## Configuration Reference

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `statusCode` | Integer | No | `200` | HTTP response status code |
| `body` | String | No | none | Response body content |
| `headers` | Object | No | none | Custom response headers (key-value pairs) |
| `contentType` | String | No | `"application/json"` | Content-Type |
| `delay` | Integer | No | none | Response delay in milliseconds |
| `terminate` | Boolean | No | `true` | Whether to terminate request processing |

### terminate Behavior

- `true` (default): Returns the mock response directly, does not forward to upstream
- `false`: Sets response status and headers but continues processing subsequent plugins and upstream forwarding

---

## Usage Scenarios

### Scenario 1: Health Check Endpoint

```yaml
requestPlugins:
  - enable: true
    type: Mock
    config:
      statusCode: 200
      body: '{"status": "healthy"}'
```

### Scenario 2: API Prototyping

```yaml
requestPlugins:
  - enable: true
    type: Mock
    config:
      statusCode: 200
      body: |
        {
          "users": [
            {"id": 1, "name": "Alice"},
            {"id": 2, "name": "Bob"}
          ]
        }
      headers:
        X-Mock: "true"
        Cache-Control: "no-cache"
```

### Scenario 3: Simulate Error Response

```yaml
requestPlugins:
  - enable: true
    type: Mock
    config:
      statusCode: 503
      body: '{"error": "Service temporarily unavailable"}'
      contentType: "application/json"
```

### Scenario 4: Delayed Response (Simulate Slow API)

```yaml
requestPlugins:
  - enable: true
    type: Mock
    config:
      statusCode: 200
      body: '{"result": "slow response"}'
      delay: 2000
```

### Scenario 5: Non-Terminating Mode

```yaml
requestPlugins:
  - enable: true
    type: Mock
    config:
      statusCode: 200
      terminate: false
      headers:
        X-Mock-Flag: "true"
```

---

## Important Notes

1. With `terminate: true`, subsequent plugins and upstream forwarding are not executed
2. Mock can be combined with Plugin Conditions for conditional mocking
3. `delay` blocks the current request processing thread; use with caution under high concurrency

---

## Complete Example

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: mock-api
spec:
  parentRefs:
    - name: my-gateway
  hostnames:
    - "mock.example.com"
  rules:
    - matches:
        - path:
            type: Exact
            value: /health
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: health-mock
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: health-mock
spec:
  requestPlugins:
    - enable: true
      type: Mock
      config:
        statusCode: 200
        body: '{"status": "ok", "version": "1.0.0"}'
        contentType: "application/json"
        headers:
          X-Health-Check: "true"
```

## Related Docs

- [Filters Overview](../overview.md)
- [Plugin Composition](../plugin-composition.md)
