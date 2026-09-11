# Bandwidth Limit Plugin

> **🔌 Edgion extension**
> 
> BandwidthLimit is a bandwidth throttling plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

## Overview

Bandwidth Limit throttles downstream response bandwidth by controlling the send rate of body chunks. Useful for preventing large file downloads from saturating bandwidth and allocating different bandwidth limits to different routes.

## Quick Start

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: bandwidth-limit-plugin
spec:
  upstreamResponseBodyFilterPlugins:
      type: BandwidthLimit
      config:
        rate: "100kb"
```

> **Note**: This plugin runs in the `upstreamResponseBodyFilterPlugins` phase, not in `requestPlugins`.

---

## Configuration Reference

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `rate` | String | **Required** | Bandwidth limit value (see Rate Format below) |

### Rate Format

| Format | Example | Description |
|--------|---------|-------------|
| Plain number | `"1024"` | Bytes per second |
| KB | `"512kb"` | Kilobytes per second |
| MB | `"1mb"` | Megabytes per second |
| GB | `"1gb"` | Gigabytes per second |

---

## Usage Scenarios

### Scenario 1: Limit Download Speed

```yaml
upstreamResponseBodyFilterPlugins:
    type: BandwidthLimit
    config:
      rate: "1mb"
```

### Scenario 2: Low Bandwidth Limit

```yaml
upstreamResponseBodyFilterPlugins:
    type: BandwidthLimit
    config:
      rate: "50kb"
```

### Scenario 3: Fractional Rate

Decimal values are supported and the unit is case-insensitive:

```yaml
upstreamResponseBodyFilterPlugins:
    type: BandwidthLimit
    config:
      # 1.5 MB/s = 1572864 bytes/s
      rate: "1.5mb"
```

---

## FAQ

### Q1: What rate formats are accepted?

**A**: A plain byte count (`"1024"` or `"1024b"`), or a number with a `kb` / `mb` / `gb` suffix (`"512kb"`, `"1mb"`, `"1gb"`). Decimals are allowed (`"1.5mb"`) and the suffix is case-insensitive (`"512KB"` works). The value is required and must parse to a positive number — an empty, zero, or unparseable rate fails validation.

### Q2: Why does total bandwidth exceed the configured rate?

**A**: The cap is applied **per response stream**, not per connection or globally. With HTTP/2 multiplexing, N concurrent streams on one connection can each receive the full `rate`, so aggregate connection bandwidth can reach N × `rate`. There is no shared token bucket across routes, connections, or pods — for aggregate caps use an external mechanism (LB QoS, kernel `tc`, or a service-mesh policy).

### Q3: Why does my BandwidthLimit have no effect?

**A**: It must be placed in the `upstreamResponseBodyFilterPlugins` phase. Putting it in `requestPlugins` or `upstreamResponseFilterPlugins` has no effect, because the throttle works by delaying upstream response body chunks.

---

## Important Notes

1. This plugin only limits downstream (client-direction) response bandwidth, not upstream request bandwidth
2. Must be placed in `upstreamResponseBodyFilterPlugins`; placing it in `requestPlugins` has no effect
3. **Scope: the rate cap is applied per response stream (per HTTP request), not per connection and not globally.**
   The plugin is stateless across chunks — each upstream body chunk yields a delay of `chunk_size / rate`,
   which Pingora applies before sending the next chunk on the same response. As a result:
   - Each in-flight response gets approximately `rate` bytes/second of throughput on its own.
   - HTTP/2 multiplexing: N concurrent streams on the same connection each get the full `rate`, so the
     aggregate connection bandwidth can reach N × `rate`.
   - HTTP/1.1 keep-alive: sequential responses on the same connection each get the full `rate` in turn.
   - There is no shared token bucket across routes, connections, or pods. For aggregate caps you need
     an external mechanism (LB-level QoS, kernel `tc`, or a service-mesh policy).

---

## Complete Example

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: download-route
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /download
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: bandwidth-limit-plugin
      backendRefs:
        - name: file-server
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: bandwidth-limit-plugin
spec:
  upstreamResponseBodyFilterPlugins:
      type: BandwidthLimit
      config:
        rate: "500kb"
```

## Related Docs

- [Rate Limit (Local)](./rate-limit-local.md)
- [Response Rewrite](./response-rewrite.md)
