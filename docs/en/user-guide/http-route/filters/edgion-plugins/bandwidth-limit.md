# Bandwidth Limit Plugin

> **🔌 Edgion Extension**
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
    - enable: true
      type: BandwidthLimit
      config:
        rate: "100kb"
```

> **Note**: This plugin runs in the `upstreamResponseBodyFilterPlugins` phase, not in `requestPlugins`.

---

## Configuration Reference

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `rate` | String | Yes | none | Bandwidth limit value |

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
  - enable: true
    type: BandwidthLimit
    config:
      rate: "1mb"
```

### Scenario 2: Low Bandwidth Limit

```yaml
upstreamResponseBodyFilterPlugins:
  - enable: true
    type: BandwidthLimit
    config:
      rate: "50kb"
```

---

## Important Notes

1. This plugin only limits downstream (client-direction) response bandwidth, not upstream request bandwidth
2. Must be placed in `upstreamResponseBodyFilterPlugins`; placing it in `requestPlugins` has no effect
3. Rate limiting is per-connection, not global

---

## Complete Example

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: download-route
spec:
  parentRefs:
    - name: my-gateway
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
    - enable: true
      type: BandwidthLimit
      config:
        rate: "500kb"
```

## Related Docs

- [Rate Limit (Local)](./rate-limit.md)
- [Response Rewrite](./response-rewrite.md)
