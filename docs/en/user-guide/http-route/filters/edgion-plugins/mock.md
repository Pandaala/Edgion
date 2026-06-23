# Mock Plugin

> **🔌 Edgion extension**
> 
> Mock is a mock response plugin provided by the `EdgionPlugins` CRD; it is not part of standard Gateway API.

## Overview

The Mock plugin returns a preset HTTP response without forwarding the request to an upstream service. Use it for API prototyping, interface testing, health-check endpoints, and error simulation.

## Quick Start

### Simplest configuration

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: mock-plugin
  namespace: production
spec:
  requestPlugins:
    - type: Mock
      config:
        statusCode: 200
        body: '{"status": "ok", "message": "Service is healthy"}'
        contentType: "application/json"
```

### Default-value notes

| Field | Default | Meaning |
|-------|---------|---------|
| `statusCode` | `200` | Returned HTTP status code |
| `contentType` | `"application/json"` | `Content-Type` of the response |
| `terminate` | `true` | Returns the mock response and stops further processing |

---

## Configuration parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `statusCode` | Integer | `200` | HTTP status code. Valid range: `100`–`599` |
| `body` | String | none | Response body content (JSON, XML, plain text, etc.) |
| `headers` | Object | none | Custom response headers as key-value pairs |
| `contentType` | String | `"application/json"` | `Content-Type` header value |
| `delay` | Integer | none | Delay in milliseconds before responding. Maximum: `30000` |
| `terminate` | Boolean | `true` | Whether to terminate request processing after running |

### `terminate` behavior

- `true` (default): returns the mock response directly; does not forward to upstream or run later plugins.
- `false`: sets the response status and headers but continues to subsequent plugins and upstream forwarding.

---

## Common configuration scenarios

### 1. Health-check endpoint

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: mock-health
  namespace: production
spec:
  requestPlugins:
    - type: Mock
      config:
        statusCode: 200
        body: '{"status": "healthy"}'
```

### 2. API prototyping with custom headers

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: mock-prototype
  namespace: production
spec:
  requestPlugins:
    - type: Mock
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

### 3. Simulate an error response

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: mock-error
  namespace: staging
spec:
  requestPlugins:
    - type: Mock
      config:
        statusCode: 503
        body: '{"error": "Service temporarily unavailable"}'
        contentType: "application/json"
```

### 4. Delayed response (simulate a slow API)

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: mock-slow
  namespace: staging
spec:
  requestPlugins:
    - type: Mock
      config:
        statusCode: 200
        body: '{"result": "slow response"}'
        # Delay is capped at 30000ms
        delay: 2000
```

### 5. Non-terminating mode (set headers, keep going)

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: mock-tag
  namespace: production
spec:
  requestPlugins:
    - type: Mock
      config:
        statusCode: 200
        terminate: false
        headers:
          X-Mock-Flag: "true"
```

---

## FAQ

### Q1: Why do my other plugins not run after Mock?

**A**: With `terminate: true` (the default), Mock returns immediately and skips all later plugins and upstream forwarding. Set `terminate: false` if you want the request to continue.

### Q2: Can I mock only some requests on a route?

**A**: Yes. Match the requests with a separate HTTPRoute rule (path / header / method match) and attach the Mock plugin only to that rule. The plugin itself always responds when it runs.

### Q3: My `delay` value seems to be ignored above 30 seconds?

**A**: `delay` is validated against a 30000ms (30s) ceiling. A configuration with a larger value is rejected at parse time.

---

## Complete configuration example

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: mock-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
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
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: health-mock
  namespace: production
spec:
  requestPlugins:
    - type: Mock
      config:
        statusCode: 200
        body: '{"status": "ok", "version": "1.0.0"}'
        contentType: "application/json"
        headers:
          X-Health-Check: "true"
```

## Related links

- [Filters overview](../README.md)
- [CORS Plugin](./cors.md)
