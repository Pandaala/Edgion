# Dynamic Upstream Plugin

> **🔌 Edgion extension**
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
      type: DynamicInternalUpstream
      config:
        from:
          type: header
          name: X-Backend-Target
        onMissing: fallback
```

### Configuration Reference

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `from` | Object | **Required** | Routing key source (see sub-fields) |
| `from.type` | String | `header` | Source type: `header` / `query` / `cookie` / `ctx` |
| `from.name` | String | none | Source name (**Required** for `header` / `query` / `cookie` / `ctx`) |
| `extract` | Object | none | Extraction rules with `regex` and `group` (default group `1`) |
| `rules` | Array\<Object\> | none | Match rules list; if unset, uses direct mode |
| `rules[].value` | String | **Required** | Routing key value to match (exact) |
| `rules[].backendRef.name` | String | **Required** | Target backendRef name (must exist in the route's `backendRefs`) |
| `rules[].backendRef.namespace` | String | same as route | Target backendRef namespace; when set, both name and namespace must match |
| `onMissing` | Enum | `Fallback` | Behavior when the routing key is missing. Allowed values: `Fallback` / `Reject` (400) |
| `onNoMatch` | Enum | `Fallback` | Behavior when no rule matches (rules mode only). Allowed values: `Fallback` / `Reject` (400) |
| `onInvalid` | Enum | `Reject` | Behavior when the resolved backendRef is not in the route. Allowed values: `Reject` (403) / `Fallback` |
| `debugHeader` | Boolean | `false` | When `true`, sets the `X-Dynamic-Internal-Upstream` request header (sent to the upstream) indicating the targeted backend |

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
        - value: "v1"
          backendRef:
            name: "api-v1"
        - value: "v2"
          backendRef:
            name: "api-v2"
        - value: "canary"
          backendRef:
            name: "api-canary"
      onNoMatch: fallback
```

#### Direct Mode with CtxSet Pre-computation

In direct mode (no `rules`), the routing key is used directly as the target backendRef name. This pairs well with the `CtxSet` plugin, which can compute the target name from arbitrary request attributes and store it in a context variable:

```yaml
requestPlugins:
  # CtxSet computes the target backend name and stores it in ctx
  - type: CtxSet
    config:
      # ... set ctx variable "target_backend" from business logic ...
  - type: DynamicInternalUpstream
    config:
      from:
        type: ctx
        name: target_backend
      # No rules => direct mode: ctx value IS the backendRef name
      onMissing: fallback
      onInvalid: reject
```

#### Canary / A-B Testing (cookie-based)

`DynamicInternalUpstream` is Edgion's equivalent of the "traffic-split" plugin found in other gateways: a request that carries a sticky cookie is pinned to a specific version, while everyone else falls through to normal weighted backend selection. Because `from.type` supports `cookie` as a first-class, by-name source, you do not need to regex the raw `Cookie` header.

```yaml
requestPlugins:
  - type: DynamicInternalUpstream
    config:
      from:
        type: cookie
        name: canary          # by-name cookie lookup, no Cookie-header regex needed
      rules:
        - value: "v2"
          backendRef:
            name: api-v2
      onMissing: fallback      # no canary cookie => normal weighted selection (e.g. 90/10)
      onNoMatch: fallback      # cookie present but unknown value => weighted selection
```

```bash
curl -H "Cookie: canary=v2" https://api.example.com/api   # pinned to api-v2
curl https://api.example.com/api                          # weighted selection
```

> **Choosing a canary strategy**
> - **Deterministic, sticky** (same user always sees the same version): use this plugin with a `cookie` / `header` source — the example above.
> - **Percentage-based gradual rollout** (e.g. shift 5% → 25% → 100%): use `backendRefs` `weight` on the route, or a plugin `conditions.run` `probability` matcher. See [Backend Weight](../../backends/weight.md).
> - **Full cutover, remotely switchable** (100% of traffic to one backend, flip it without redeploying): use the [Canary plugin](./canary.md) with an `activeProfileRef` Selector.

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

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `from` | Object | **Required** | Value source (see sub-fields) |
| `extract` | Object | none | Extraction rules with `regex` and `group` (default group `1`) |
| `domainMap` | Object | **Required** | Domain mapping whitelist (must be non-empty) |
| `onMissing` | Enum | `Skip` | Behavior when the routing key is missing. Allowed values: `Skip` / `Reject` (400) |
| `onNoMatch` | Enum | `Skip` | Behavior when the key matches no `domainMap` entry. Allowed values: `Skip` / `Reject` (400) |
| `debugHeader` | Boolean | `false` | When `true`, sets the `X-Dynamic-External-Upstream` request header (sent to the upstream) indicating the targeted domain |

### DomainTarget Sub-fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `domain` | String | **Required** | Target domain (hostname; rejects spaces, `/`, `\`, `@`, `#`, `?`, `[`, `]`, and control characters) |
| `port` | Integer | `443` if `tls: true`, else `80` | Target port |
| `tls` | Boolean | `true` | Use TLS for the upstream connection |
| `overrideHost` | String | original request Host | Override the Host header sent to the external domain |
| `sni` | String | the `domain` value | Override the TLS SNI (only relevant when `tls: true`) |

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

#### Third-Party API Proxy (Override Host + SNI)

Route to an external partner API where the upstream expects its own hostname in the Host header and presents a certificate for a different name:

```yaml
requestPlugins:
  - type: DynamicExternalUpstream
    config:
      from:
        type: query
        name: provider
      domainMap:
        "provider-a":
          domain: api-a.example.com
          port: 443
          tls: true
          # Upstream routes by its own hostname
          overrideHost: api-a.example.com
        "provider-b":
          domain: gateway.example.com
          port: 8443
          tls: true
          overrideHost: api-b.example.com
          # Certificate CN differs from the connection domain
          sni: api-b.example.com
      onMissing: reject
      onNoMatch: reject
```

---

## Behavior Details

- **DynamicInternalUpstream**: The selected target must exist in the current route's `backendRefs`; otherwise it is treated as invalid
- **DynamicExternalUpstream**: Can only route to domains in the `domainMap` whitelist, preventing SSRF
- In `Skip` / `Fallback` mode, unmatched requests use normal routing logic
- With `debugHeader: true`, a request header (`X-Dynamic-Internal-Upstream` or `X-Dynamic-External-Upstream`) is added and forwarded to the upstream, indicating the actual routed target

---

## FAQ

### Q1: What is the difference between `onNoMatch` and `onInvalid` for DynamicInternalUpstream?

**A**: `onNoMatch` applies in rules mode when the routing key matches none of the `rules` entries (default `Fallback`, returning normal weighted selection). `onInvalid` applies after a target is resolved but its name is not present in the route's `backendRefs` (default `Reject` with 403, since this usually indicates a misconfiguration). In direct mode there are no rules, so a non-existent target name falls through to `onInvalid`.

### Q2: Why does my DynamicExternalUpstream request go to the normal backend instead of the external domain?

**A**: The default `onMissing` and `onNoMatch` are both `Skip`, which proceeds with normal backend selection rather than rejecting. If the routing key is missing or matches no `domainMap` entry, the request silently uses the route's `backendRefs`. Set both to `Reject` to fail closed, or enable `debugHeader: true` to confirm whether a jump occurred.

### Q3: Can DynamicExternalUpstream reach any domain?

**A**: No. Only domains explicitly listed in `domainMap` can be targeted (whitelist model). This prevents SSRF — a client cannot supply an arbitrary domain through the routing key.

---

## Related Docs

- [Direct Endpoint](./direct-endpoint.md)
- [ProxyRewrite](./proxy-rewrite.md)
- [Backends](../../backends/README.md)
- [Filters Overview](../overview.md)
