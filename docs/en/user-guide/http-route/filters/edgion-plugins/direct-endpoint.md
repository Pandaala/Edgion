# Direct Endpoint Plugin

> **🔌 Edgion Extension**
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
| `extract.regex` | String | Yes | none | Regular expression |
| `extract.group` | Integer | Yes | none | Capture group index |
| `port` | Integer | No | none | Override port number |
| `onMissing` | String | No | `Fallback` | Behavior when value is missing: `Fallback` / `Reject` |
| `onInvalid` | String | No | `Reject` | Behavior when value is invalid: `Reject` / `Fallback` |
| `inheritTls` | Boolean | No | `true` | Inherit route's TLS configuration |
| `debugHeader` | Boolean | No | `false` | Add debug header to response |

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
- If the endpoint doesn't belong to any backend, it is handled as `onInvalid`
- With `debugHeader: true`, the response includes an `X-Direct-Endpoint` header indicating the actual routed endpoint
- `Fallback` mode falls back to normal load balancing selection

---

## Troubleshooting

### Problem 1: Specified IP Routes to Different Instance

**Cause**: The specified IP doesn't belong to the current route's backend Service.

**Solution**:
```bash
kubectl get endpoints <service-name> -o yaml
```

### Problem 2: Returns 400 Error

**Cause**: `onInvalid` is set to `Reject` and the provided value has invalid format.

**Solution**: Ensure a valid IP address format is provided.

---

## Related Docs

- [Dynamic Upstream](./dynamic-upstream.md)
- [Load Balancing Algorithms](../../lb-algorithms.md)
- [Backends](../../backends/README.md)
