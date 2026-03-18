# Request Mirror Plugin

> **🔌 Edgion Extension**
> 
> Edgion supports both the standard Gateway API `RequestMirror` filter and the reusable `EdgionPlugins` `RequestMirror` configuration.
> This page focuses on the reusable `EdgionPlugins` form and the shared runtime behavior behind both entry points.

## Overview

Request Mirror asynchronously mirrors inbound requests to another backend service without affecting the primary request processing. Useful for traffic replication, canary testing, and request auditing.

**Features**:
- Asynchronous mirroring, does not block primary request
- Supports fractional traffic sampling
- Mirror results can be recorded in access logs
- Concurrency limits to prevent mirror target overload

## Quick Start

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: mirror-plugin
spec:
  requestPlugins:
    - enable: true
      type: RequestMirror
      config:
        backendRef:
          name: mirror-service
          port: 8080
```

---

## Configuration Reference

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `backendRef` | Object | Yes | none | Mirror backend reference |
| `backendRef.name` | String | Yes | none | Backend service name |
| `backendRef.namespace` | String | No | same as route | Backend service namespace |
| `backendRef.port` | Integer | No | none | Backend service port |
| `fraction` | Object | No | none (100%) | Mirror traffic fraction |
| `fraction.numerator` | Integer | Yes | none | Numerator |
| `fraction.denominator` | Integer | Yes | none | Denominator |
| `connectTimeoutMs` | Integer | No | `1000` | Connection timeout (ms) |
| `writeTimeoutMs` | Integer | No | `1000` | Write timeout (ms) |
| `maxBufferedChunks` | Integer | No | `5` | Maximum buffered chunks |
| `mirrorLog` | Boolean | No | `true` | Log mirror results in access log |
| `maxConcurrent` | Integer | No | `1024` | Maximum concurrent mirrors |
| `channelFullTimeoutMs` | Integer | No | `0` | Channel full wait timeout (ms) |

---

## Usage Scenarios

### Scenario 1: Full Mirror to Test Service

```yaml
requestPlugins:
  - type: RequestMirror
    config:
      backendRef:
        name: test-service
        namespace: testing
        port: 8080
      mirrorLog: true
```

### Scenario 2: Fractional Traffic Sampling

Mirror only 10% of traffic:

```yaml
requestPlugins:
  - type: RequestMirror
    config:
      backendRef:
        name: analytics-service
        port: 8080
      fraction:
        numerator: 1
        denominator: 10
```

### Scenario 3: Concurrency Limiting

```yaml
requestPlugins:
  - type: RequestMirror
    config:
      backendRef:
        name: mirror-service
        port: 8080
      maxConcurrent: 50
      connectTimeoutMs: 500
      writeTimeoutMs: 500
```

---

## Important Notes

1. Mirroring is asynchronous and does not affect primary request latency or status code
2. Mirror failures do not cause the primary request to fail
3. Request body is also mirrored; `maxBufferedChunks` controls buffer size
4. When concurrent mirrors reach `maxConcurrent`, new mirror requests are dropped

---

## Related Docs

- [ProxyRewrite](./proxy-rewrite.md)
- [Filters Overview](../overview.md)
