# Request Mirror Plugin

> **🔌 Edgion extension**
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
| `backendRef.group` | String | No | `""` (core) | API group of the referent |
| `backendRef.kind` | String | No | `Service` | Kind of the referent |
| `backendRef.namespace` | String | No | same as route | Backend service namespace |
| `backendRef.port` | Integer | No | none | Backend service port |
| `fraction` | Object | No | none (100%) | Mirror traffic fraction (Gateway API style) |
| `fraction.numerator` | Integer | Yes (within `fraction`) | none | Numerator |
| `fraction.denominator` | Integer | No | `100` | Denominator |
| `percentage` | Integer | No | none (100%) | Percentage of requests to mirror (0–100). Gateway API v1.5 field, also accepted as `percent` |
| `connectTimeoutMs` | Integer | No | `1000` | Connection timeout (ms) |
| `writeTimeoutMs` | Integer | No | `1000` | Write timeout (ms) |
| `maxBufferedChunks` | Integer | No | `5` | Maximum body chunks buffered when the mirror is slower than the client |
| `mirrorLog` | Boolean | No | `true` | Emit a dedicated mirror line in the access log |
| `maxConcurrent` | Integer | No | `1024` | Maximum concurrent mirror tasks per gateway process |
| `channelFullTimeoutMs` | Integer | No | `0` | Max ms to wait for mirror channel space before abandoning the mirror. `0` = abandon immediately (zero impact on the main request) |

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

### Scenario 3: Percentage-Based Sampling

`percentage` is an alternative to `fraction` (Gateway API v1.5 style). Mirror 5% of traffic:

```yaml
requestPlugins:
  - type: RequestMirror
    config:
      backendRef:
        name: analytics-service
        port: 8080
      # Equivalent to fraction: { numerator: 5, denominator: 100 }
      percentage: 5
```

### Scenario 4: Concurrency Limiting

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
5. `channelFullTimeoutMs: 0` (default) abandons the mirror immediately when the mirror body channel is full, guaranteeing zero impact on the main request; a small positive value (100–500ms) grants a brief back-pressure window before abandoning

---

## FAQ

### Q1: What is the difference between `fraction` and `percentage`?

**A**: They are two ways to express the same sampling ratio. `fraction` uses `numerator` / `denominator` (Gateway API classic style); `percentage` is an integer 0–100 (Gateway API v1.5 style, also accepted under the field name `percent`). Use whichever is more convenient — set only one.

### Q2: Can a slow mirror backend slow down my real traffic?

**A**: No, by default. With `channelFullTimeoutMs: 0` the gateway abandons the mirror the moment its buffer is full rather than waiting. If you want to give a briefly-lagging mirror a chance to catch up, set `channelFullTimeoutMs` to a small value (e.g. `200`); beyond that window the mirror is dropped and the main request proceeds normally.

### Q3: Is this the same as the Gateway API `RequestMirror` filter?

**A**: Yes — both entry points share the same runtime. You can configure mirroring inline via the standard Gateway API `RequestMirror` filter, or as a reusable `EdgionPlugins` `RequestMirror` entry referenced through `ExtensionRef`. This page documents the reusable form.

---

## Related Docs

- [ProxyRewrite](./proxy-rewrite.md)
- [Filters Overview](../overview.md)
