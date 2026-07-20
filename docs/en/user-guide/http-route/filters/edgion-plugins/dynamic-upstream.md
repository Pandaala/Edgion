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

`dye` is an **entry-level** field — a sibling of `type`/`config`/`enable`, not a member of `config`:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `dye` | Object | none | Traffic-tagging headers keyed on outcome (`on: [success, fallback, reject]`). `request`/`response` entry lists of `{name, on, value, conditions}`; an omitted `value` carries the resolved target's `[ns/]name` on `success`. See [Dye](./dye.md) for the full reference, and [Dye Headers](#dye-headers) below for the plugin-specific summary. |

### Usage Scenarios

#### Direct Mode

Header value is used directly as the target Service name:

```yaml
requestPlugins:
  - type: DynamicInternalUpstream
    dye:
      request:
        - name: X-Dynamic-Internal-Upstream
    config:
      from:
        type: header
        name: X-Backend-Target
      onMissing: fallback
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

`dye` is an **entry-level** field — a sibling of `type`/`config`/`enable`, not a member of `config`:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `dye` | Object | none | Traffic-tagging headers keyed on outcome (`on: [success, fallback, reject]`). `request`/`response` entry lists of `{name, on, value, conditions}`; an omitted `value` carries the resolved target's `domain:port` on `success`. See [Dye](./dye.md) for the full reference, and [Dye Headers](#dye-headers) below for the plugin-specific summary. |

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
    dye:
      request:
        - name: X-Dynamic-External-Upstream
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

## Dye Headers

See [Dye](./dye.md) for the full reference: the `on` outcome vocabulary, the two `value` provenances, the template engine, the anti-spoof strip (runs even for `enable: false` and condition-skipped entries), header-name validation, and security guidance. This section covers only what is specific to these two plugins.

`dye` is default-off: when omitted, neither plugin sets any header or strips anything. When configured, it tags the request/response with operator-chosen HTTP headers:

```yaml
requestPlugins:
  - type: DynamicInternalUpstream
    dye:
      request:
        - name: X-Release-Track      # value omitted => resolved target "[ns/]name" (on: [success] by default)
      response:
        - name: X-Served-By
          value: edge-cluster-1      # explicit static value
        - name: X-Edgion-Outcome
          on: [fallback, reject]     # value omitted => dynamic_internal_upstream:fallback:on-missing, etc.
    config:
      from:
        type: header
        name: X-Backend-Target
```

- An omitted `value` on `success` carries the resolved selection: **`DynamicInternalUpstream`** uses the target backendRef's `[ns/]name`; **`DynamicExternalUpstream`** uses the target's `domain:port`.
- Both plugins declare `success`, `fallback`, and `reject` dye capabilities — `fallback`/`reject` rules also dye the `onMissing`/`onNoMatch` branches (whichever of `fallback`/`skip` or `reject` each is configured to take), and, for `DynamicInternalUpstream` only, the `onInvalid` branch (DynamicExternalUpstream has no `onInvalid`, since `domainMap` membership is inherent to matching). A plain `success` rule (the default `on`) never fires on those paths.
- Protocol gating follows the plugins' existing gRPC skip: neither plugin *dyes* gRPC traffic, since both early-return on a gRPC-matched route before reaching the point where they would record an outcome. The **strip** still runs on gRPC traffic — it is runtime-owned and unconditional (see [Dye](./dye.md)), so a client cannot use a gRPC request to smuggle a spoofed dye header past a route that also serves HTTP.

## Behavior Details

- **DynamicInternalUpstream**: The selected target must exist in the current route's `backendRefs`; otherwise it is treated as invalid
- **DynamicExternalUpstream**: Can only route to domains in the `domainMap` whitelist, preventing SSRF
- In `Skip` / `Fallback` mode, unmatched requests use normal routing logic
- See [Dye Headers](#dye-headers) above for the `dye` behavior shared by both sub-plugins

---

## FAQ

### Q1: What is the difference between `onNoMatch` and `onInvalid` for DynamicInternalUpstream?

**A**: `onNoMatch` applies in rules mode when the routing key matches none of the `rules` entries (default `Fallback`, returning normal weighted selection). `onInvalid` applies after a target is resolved but its name is not present in the route's `backendRefs` (default `Reject` with 403, since this usually indicates a misconfiguration). In direct mode there are no rules, so a non-existent target name falls through to `onInvalid`.

### Q2: Why does my DynamicExternalUpstream request go to the normal backend instead of the external domain?

**A**: The default `onMissing` and `onNoMatch` are both `Skip`, which proceeds with normal backend selection rather than rejecting. If the routing key is missing or matches no `domainMap` entry, the request silently uses the route's `backendRefs`. Set both to `Reject` to fail closed, or configure `dye` to confirm whether a jump occurred.

### Q3: Can DynamicExternalUpstream reach any domain?

**A**: No. Only domains explicitly listed in `domainMap` can be targeted (whitelist model). This prevents SSRF — a client cannot supply an arbitrary domain through the routing key.

---

## Related Docs

- [Direct Endpoint](./direct-endpoint.md)
- [ProxyRewrite](./proxy-rewrite.md)
- [Backends](../../backends/README.md)
- [Filters Overview](../overview.md)
