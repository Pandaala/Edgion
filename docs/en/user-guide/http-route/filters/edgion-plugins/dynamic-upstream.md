# Dynamic Upstream Plugin

> **🔌 Edgion Extension**
> 
> DynamicInternalUpstream and DynamicExternalUpstream are dynamic upstream routing plugins provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

## Overview

Dynamic Upstream allows dynamically selecting upstream targets based on request metadata (Header / Query / Cookie). It includes two sub-types:

- **DynamicInternalUpstream**: Dynamically selects a specific Service from the current route's existing `backendRefs`, bypassing weighted selection
- **DynamicExternalUpstream**: Routes traffic to external domains via a domain map whitelist

## DynamicInternalUpstream

### Quick Start

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: diu-plugin
spec:
  requestPlugins:
    - enable: true
      type: DynamicInternalUpstream
      config:
        from:
          type: header
          name: X-Backend-Target
        onMissing: fallback
```

### Configuration Reference

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `from` | Object | Yes | `{type:"header",name:"X-Backend-Target"}` | Value source |
| `from.type` | String | Yes | `header` | Source type: `header` / `query` / `cookie` / `ctx` |
| `from.name` | String | Yes | none | Source name |
| `extract` | Object | No | none | Extraction rules with `regex` and `group` |
| `rules` | Array | No | none | Match rules list; if unset, uses direct mode |
| `onMissing` | String | No | `Fallback` | Behavior when value is missing: `Fallback` / `Reject` |
| `onNoMatch` | String | No | `Fallback` | Behavior when no rule matches: `Fallback` / `Reject` |
| `onInvalid` | String | No | `Reject` | Behavior when value is invalid: `Reject` / `Fallback` |
| `debugHeader` | Boolean | No | `false` | Add debug header |

### Usage Scenarios

#### Direct Mode

Header value is used directly as the target Service name:

```yaml
requestPlugins:
  - type: DynamicInternalUpstream
    config:
      from:
        type: header
        name: X-Backend-Target
      onMissing: fallback
      debugHeader: true
```

```bash
curl -H "X-Backend-Target: service-v2" https://api.example.com/api
```

#### Rules Mode

Map header values to target Services via rules:

```yaml
requestPlugins:
  - type: DynamicInternalUpstream
    config:
      from:
        type: header
        name: X-Version
      rules:
        - match: "v1"
          target: "api-v1"
        - match: "v2"
          target: "api-v2"
        - match: "canary"
          target: "api-canary"
      onNoMatch: fallback
```

---

## DynamicExternalUpstream

### Quick Start

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: deu-plugin
spec:
  requestPlugins:
    - enable: true
      type: DynamicExternalUpstream
      config:
        from:
          type: header
          name: X-Target-Region
        domainMap:
          "us-west":
            domain: us-west.api.internal
            port: 443
            tls: true
          "eu-central":
            domain: eu-central.api.internal
            port: 443
            tls: true
```

### Configuration Reference

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `from` | Object | Yes | `{type:"header",name:"X-Target-Region"}` | Value source |
| `extract` | Object | No | none | Extraction rules |
| `domainMap` | Object | Yes | none | Domain mapping whitelist |
| `onMissing` | String | No | `Skip` | Behavior when value is missing: `Skip` / `Reject` |
| `onNoMatch` | String | No | `Skip` | Behavior when no match: `Skip` / `Reject` |
| `debugHeader` | Boolean | No | `false` | Add debug header |

### DomainTarget Sub-fields

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `domain` | String | Yes | none | Target domain |
| `port` | Integer | No | none | Target port |
| `tls` | Boolean | No | `true` | Use TLS |
| `overrideHost` | String | No | none | Override Host header |
| `sni` | String | No | none | TLS SNI name |

### Usage Scenarios

#### Multi-Region Routing

```yaml
requestPlugins:
  - type: DynamicExternalUpstream
    config:
      from:
        type: header
        name: X-Cluster-Target
      extract:
        regex: 'cluster=([\w-]+)'
        group: 1
      domainMap:
        "us-west":
          domain: us-west.api.internal
          port: 443
          tls: true
          overrideHost: api.example.com
        "eu-central":
          domain: eu-central.api.internal
          port: 443
          tls: true
          overrideHost: api.example.com
        "ap-east":
          domain: ap-east.api.internal
          port: 443
          tls: true
      onMissing: skip
      debugHeader: true
```

---

## Behavior Details

- **DynamicInternalUpstream**: The selected target must exist in the current route's `backendRefs`; otherwise it is treated as invalid
- **DynamicExternalUpstream**: Can only route to domains in the `domainMap` whitelist, preventing SSRF
- In `Skip` / `Fallback` mode, unmatched requests use normal routing logic
- With `debugHeader: true`, the response includes a header indicating the actual routed upstream

---

## Related Docs

- [Direct Endpoint](./direct-endpoint.md)
- [Load Balancing Algorithms](../../lb-algorithms.md)
- [ProxyRewrite](./proxy-rewrite.md)
