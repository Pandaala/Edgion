# Debug Access Log Plugin

> **🔌 Edgion extension**
> 
> DebugAccessLogToHeader is a debugging plugin provided by the `EdgionPlugins` CRD; it is not part of standard Gateway API.

## Overview

The DebugAccessLogToHeader plugin serializes the request's access log to JSON and injects it into a response header. Use it to inspect routing, timing, and upstream details directly from a browser network tab or `curl -v`, without reading gateway logs.

The injected header is `X-Debug-Access-Log`.

⚠️ **For development only! Do not use in production!** The access log may contain sensitive routing and upstream information, and it inflates every response.

## Quick Start

### Simplest configuration

This plugin runs in the upstream-response-filter phase (it needs the response to exist before it can attach the header), so place it in `upstreamResponseFilterPlugins` — not `requestPlugins`.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: debug-plugin
  namespace: staging
spec:
  upstreamResponseFilterPlugins:
    - type: DebugAccessLogToHeader
      config: {}
```

---

## Configuration parameters

This plugin takes no configuration parameters. Enable it by adding the entry with an empty `config: {}`.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| _(none)_ | — | — | No parameters; the plugin always injects the full access log |

---

## How it works

1. The request is proxied to the upstream as normal.
2. In the upstream-response-filter phase, the plugin serializes the access log (the same structured record the gateway would write to its access log) to a JSON string.
3. It adds that JSON string to the response under the `X-Debug-Access-Log` header.

Inspect it on the client side:

```bash
curl -v https://api.example.com/path 2>&1 | grep -i x-debug-access-log
```

The header value contains fields such as the matched route, upstream endpoint, status code, and timing — exactly what the access log records for that request.

---

## Common configuration scenarios

### 1. Enable on a single debugging route

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: debug-single
  namespace: staging
spec:
  upstreamResponseFilterPlugins:
    - type: DebugAccessLogToHeader
      config: {}
```

### 2. Temporarily disable without removing the resource

The `enable` flag (default `true`) lets you toggle the entry off while keeping it in the manifest.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: debug-toggle
  namespace: staging
spec:
  upstreamResponseFilterPlugins:
    - type: DebugAccessLogToHeader
      enable: false
      config: {}
```

### 3. Combine with other response-filter plugins

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: debug-combo
  namespace: staging
spec:
  upstreamResponseFilterPlugins:
    - type: ResponseHeaderModifier
      config:
        add:
          - name: X-Env
            value: staging
    - type: DebugAccessLogToHeader
      config: {}
```

---

## FAQ

### Q1: I added the plugin but no `X-Debug-Access-Log` header appears?

**A**: Make sure the entry is under `upstreamResponseFilterPlugins`. The plugin only runs in the upstream-response-filter phase. If you place it under `requestPlugins`, its filter is never invoked.

### Q2: Is it safe to leave this on in production?

**A**: No. The access log may expose internal routing and upstream details, and the extra header bloats every response. Scope it to staging or a temporary debugging route.

---

## Complete configuration example

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: debug-route
  namespace: staging
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - "api.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: debug-access-log
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: debug-access-log
  namespace: staging
spec:
  upstreamResponseFilterPlugins:
    - type: DebugAccessLogToHeader
      config: {}
```

## Related links

- [Filters overview](../README.md)
- [Mock Plugin](./mock.md)
