# RegionRoute Plugin

> **🔌 Edgion extension**
> 
> RegionRoute is a multi-region traffic routing plugin provided by the `EdgionPlugins` CRD and is not part of the standard Gateway API.

## Overview

RegionRoute routes traffic across multiple regions (clusters) using ordered route rules: direct region override, key-to-region mapping, and consistent hash-range routing with optional failover.

In the `EdgionPlugins` CR the plugin config holds **only** a `serviceRegionRouteRef`. All routing rules — clusters, hash ranges, and failover state — live in `PluginMetaData` resources. At plugin-build time the Gateway resolves the chain:

```text
EdgionPlugins.serviceRegionRouteRef
  → ServiceRegionRoute (PluginMetaData)
    → ServiceRegionRoute.clusterRef → ClusterRegionRoute (PluginMetaData)
```

The `ServiceRegionRoute` and `ClusterRegionRoute` PluginMetaData resources must be pre-created by an administrator (the `ClusterRegionRoute` is typically managed by Center, which also writes per-cluster failover state).

## Quick Start

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: example-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
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
            name: my-region-route-plugin
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-region-route-plugin
  namespace: production
spec:
  requestPlugins:
    - type: RegionRoute
      config:
        # Reference a ServiceRegionRoute PluginMetaData in the same namespace
        serviceRegionRouteRef: my-svc-region-route
```

---

## Configuration Parameters

### Plugin config (`EdgionPlugins`)

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `serviceRegionRouteRef` | String | none | Name of a `ServiceRegionRoute` PluginMetaData in the same namespace as the `EdgionPlugins` resource. All routing rules are resolved from this reference |

The plugin performs no local field validation — existence of the referenced PluginMetaData is checked by the Controller's `preparse()` (surfaced via status conditions) and by the Gateway at plugin-build time.

### Routing model (PluginMetaData)

The referenced PluginMetaData defines the route rules, evaluated **in order — first match wins**:

| Rule type | Behavior |
|-----------|----------|
| `RouteByDirectRegion` | Extracts a region name directly from the request (via `keyFrom` sources, in order); the first value matching a defined region is used. Non-matching values fall through |
| `RouteByKeyConfMatch` | Extracts a key and looks it up in a fixed `matchMap` (key → region). On match, routes to the mapped region; otherwise falls through |
| `RouteByHashRange` | Consistent hash-range routing. Reads a pre-computed hash if present and valid (in `[0, modulo)`), otherwise computes it. This rule is **terminal** — it never falls through to later rules |

Each region (cluster) defines:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | String | — | Cluster identifier (used in direct-region matching and failover) |
| `hashRange` | `[Integer, Integer]` | — | Closed hash interval `[low, high]`; must satisfy `0 ≤ low ≤ high < modulo` |
| `backendEndpoint` | String | — | Target endpoint `host` or `host:port`. Only the upstream IP changes; the original SNI and request headers are preserved |
| `tls` | Boolean | `true` | Whether the upstream connection uses TLS (port defaults to `443` when `true`, `80` when `false`) |
| `failoverTo` | String | none | Redirect traffic for this cluster to the named cluster; must be a valid cluster in the same list and cannot be itself |

Hash computation:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `algorithm` | Enum | `Crc32` | Hash algorithm: `Crc32` (fast, hardware-accelerated) or `Sha256` |
| `modulo` | Integer | `1000` | Hash value domain `[0, modulo-1]`; every region's `hashRange[1]` must be `< modulo` |
| `writeBack` | Array | none | Targets to which the computed hash is written back (empty = no write-back) |

---

## Common Configuration Scenarios

### 1. Pin Specific Tenants to a Region (direct mapping)

The PluginMetaData uses a `RouteByKeyConfMatch` rule that maps a header value to a region, so named tenants always land in the same cluster. The `EdgionPlugins` config stays minimal:

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: tenant-region-route
  namespace: production
spec:
  requestPlugins:
    - type: RegionRoute
      config:
        serviceRegionRouteRef: tenant-region-route-meta
```

### 2. Consistent Hash-Range Distribution

The PluginMetaData defines `RouteByHashRange` with per-cluster `hashRange` slices over `modulo` (e.g. `0–499` and `500–999` for a 50/50 split). A request whose computed hash falls in a cluster's range is routed there. Because `RouteByHashRange` is terminal, place it last.

### 3. Region Failover

Set `failoverTo` on a cluster in the `ClusterRegionRoute` PluginMetaData (typically driven by Center). Traffic destined for a degraded cluster is transparently redirected to the named healthy cluster — the `EdgionPlugins` and HTTPRoute do not change.

---

## FAQ

### Q1: Why is there no routing configuration in the EdgionPlugins resource?

**A**: By design. The `EdgionPlugins` config holds only `serviceRegionRouteRef`. Keeping clusters, hash ranges, and failover state in PluginMetaData lets operators (and Center) update routing centrally without editing every route, and lets multiple routes share one `ServiceRegionRoute`.

### Q2: What is the order of rule evaluation?

**A**: Rules are evaluated top to bottom, first match wins. `RouteByDirectRegion` and `RouteByKeyConfMatch` fall through when they do not match, so put more specific rules first. `RouteByHashRange` is terminal and should be last.

### Q3: My route falls back even though I configured RegionRoute. Why?

**A**: The most common cause is a missing or unresolved PluginMetaData chain (`ServiceRegionRoute` → `ClusterRegionRoute`). Check the HTTPRoute / EdgionPlugins status conditions and the gateway logs — the Controller reports a missing reference there.

---

## Related Documentation

- [Dynamic Upstream](./dynamic-upstream.md) — Request-driven upstream selection
- [Direct Endpoint](./direct-endpoint.md) — Bypass load balancing to a specific endpoint
- [Filters Overview](../overview.md) — All HTTPRoute filters
