# Debug Access Log Plugin

> **🔌 Edgion extension**
> 
> DebugAccessLogToHeader is a debugging plugin provided by the `EdgionPlugins` CRD; it is not part of standard Gateway API.

## Overview

The DebugAccessLogToHeader plugin serializes the request's current, non-terminal access-log snapshot to JSON and injects it into a response header. Use it to inspect routing and upstream-response-header-stage details directly from a browser network tab or `curl -v`, without reading gateway logs.

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
| _(none)_ | — | — | No plugin parameters; header exposure follows the gateway's global `access_log_sensitive.mode` policy |

---

## How it works

1. The request is proxied to the upstream as normal.
2. In the upstream-response-filter phase, the plugin serializes the request state available at that moment to a JSON string.
3. When the global sensitive mode permits exposure, it adds that JSON string to the response under the `X-Debug-Access-Log` header.

Inspect it on the client side:

```bash
curl -v https://api.example.com/path 2>&1 | grep -i x-debug-access-log
```

### Snapshot boundary

The header is a response-header-stage snapshot, not the terminal access-log record. Its top-level `request_info.status` is always `0` because terminal `logging()` finalizes that field later. Separately, `backend_context.upstreams[].status` may already contain the status observed from an upstream attempt.

The snapshot always omits `ext` and `waf`. Response-body and elapsed timing, final AI or streaming summaries, late diagnostics, and the current or later response-stage plugin logs may be absent or unfinished. Use the terminal access log for the complete request outcome.

### Sensitive-mode behavior

The gateway-wide `access_log_sensitive.mode` setting controls whether the header can be exposed:

| Mode | Header behavior |
|------|-----------------|
| `replace` (default) | Emits parseable JSON with sensitive fields replaced by the configured replacement string |
| `off` | Emits parseable JSON with unmasked, unmarked raw values; this is an explicit debugging opt-out |
| `wrap` | Emits no debug header because wrapped values retain raw data; request processing continues, and `wrap mode:header suppressed` appears later in the terminal access log |

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

**A**: Make sure the entry is under `upstreamResponseFilterPlugins`. The plugin only runs in the upstream-response-filter phase. If it is configured correctly but the gateway uses `access_log_sensitive.mode: wrap`, the missing header is expected; check the terminal access log for `wrap mode:header suppressed`.

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
