# Direct Endpoint Plugin

> **🔌 Edgion extension**
> 
> DirectEndpoint is a direct endpoint routing plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

## Overview

Direct Endpoint allows routing requests directly to a specific endpoint IP via request metadata (Header / Query / Cookie), bypassing load balancing algorithms. The specified endpoint must belong to a Service in the current route's `backendRefs`.

Useful for debugging, targeted testing, and session affinity scenarios.

## Quick Start

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: direct-endpoint-plugin
spec:
  requestPlugins:
    - enable: true
      type: DirectEndpoint
      config:
        from:
          type: header
          name: X-Target-Endpoint
```

---

## Configuration Reference

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `from` | Object | Yes | `{type:"header",name:"X-Target-Endpoint"}` | Endpoint value source |
| `from.type` | String | Yes | `header` | Source type: `header` / `query` / `cookie` / `ctx` |
| `from.name` | String | Yes | `X-Target-Endpoint` | Source name |
| `extract` | Object | No | none | Extraction rules |
| `extract.regex` | String | Yes (within `extract`) | none | Regular expression applied to the raw value |
| `extract.group` | Integer | No | `1` | Capture group index (`0` = full match) |
| `port` | Integer | No | none | Port to use when the value carries no `ip:port` and the backendRef has none |
| `onMissing` | String | No | `Fallback` | Behavior when the value is missing: `Fallback` (normal LB) / `Reject` (400) |
| `onInvalid` | String | No | `Reject` | Behavior when the endpoint is not in any backendRef: `Reject` (403) / `Fallback` |
| `inheritTls` | Boolean | No | `true` | Inherit TLS from the matched backendRef's BackendTLSPolicy |
| `debugHeader` | Boolean | No | `false` | When `true`, sets the `X-Direct-Endpoint` request header (sent to the upstream) indicating the targeted endpoint |

---

## Usage Scenarios

### Scenario 1: Specify Target via Header

```yaml
requestPlugins:
  - type: DirectEndpoint
    config:
      from:
        type: header
        name: X-Target-IP
      onMissing: fallback
      onInvalid: reject
```

**Test**:
```bash
curl -H "X-Target-IP: 10.0.1.5" https://api.example.com/debug
```

### Scenario 2: Debug Mode

```yaml
requestPlugins:
  - type: DirectEndpoint
    config:
      from:
        type: header
        name: X-Target-IP
      debugHeader: true
      onMissing: fallback
```

### Scenario 3: Regex Extraction

```yaml
requestPlugins:
  - type: DirectEndpoint
    config:
      from:
        type: header
        name: X-Target-Info
      extract:
        regex: 'ip=([0-9.]+)'
        group: 1
      port: 8080
```

---

## Behavior Details

- The specified endpoint IP must belong to the Endpoints list of a Service in the current route's `backendRefs`
- If the endpoint doesn't belong to any backend, it is handled as `onInvalid` (default `Reject` with 403)
- The port is resolved in priority order: (1) port embedded in the value (`ip:port`), (2) the `port` field, (3) the matched backendRef's port. If none are available, the plugin returns an error
- With `debugHeader: true`, the `X-Direct-Endpoint` request header is added and forwarded to the upstream, indicating the actual routed endpoint
- `Fallback` mode falls back to normal load balancing selection

---

## Troubleshooting

### Problem 1: Specified IP Routes to Different Instance

**Cause**: The specified IP doesn't belong to the current route's backend Service.

**Solution**:
```bash
kubectl get endpoints <service-name> -o yaml
```

### Problem 2: Returns 400 Bad Request

**Cause**: The endpoint value is missing from the request and `onMissing` is set to `Reject`.

**Solution**: Provide the configured source (header/query/cookie/ctx), or set `onMissing: fallback` to use normal load balancing when the value is absent.

### Problem 3: Returns 403 Forbidden

**Cause**: The resolved endpoint is not part of any of the route's `backendRefs` and `onInvalid` is `Reject` (the default).

**Solution**: Confirm the endpoint IP belongs to a backend Service's Endpoints, or set `onInvalid: fallback`.

---

## Related Docs

- [Dynamic Upstream](./dynamic-upstream.md)
- [Backends](../../backends/README.md)
- [Filters Overview](../overview.md)
