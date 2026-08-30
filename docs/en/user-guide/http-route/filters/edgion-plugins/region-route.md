# RegionRoute Plugin

> **🔌 Edgion extension**
> 
> RegionRoute is a multi-region traffic routing plugin provided by the `EdgionPlugins` CRD and is not part of the standard Gateway API.

## Overview

RegionRoute routes traffic across multiple regions (clusters) using ordered route rules: direct region override, key-to-region mapping, and consistent hash-range routing with optional failover.

All routing logic — the route rules, key extraction, hash computation — **and** the default region table live in the `EdgionPlugins` plugin config itself. That config is self-sufficient: a RegionRoute plugin with no external reference at all is a complete, working configuration.

An optional `overrideRef` points at an `EdgionConfigData` of type `RegionRouteOverride`. It is a **data-only overlay**: it can replace the region table, but it can never change routing logic. The Gateway resolves it **at request time**, so an operator (or Center) can swap the region table without touching the `EdgionPlugins` resource:

```text
EdgionPlugins.requestPlugins[].config.overrideRef
  → EdgionConfigData (spec.data.type: RegionRouteOverride)
      regions?      # data only — never route rules
```

If the overlay is absent, disabled, of the wrong type, or produces an invalid region table, the plugin uses the base `regions` from `EdgionPlugins` — **fail-to-base**, never fail-closed. See [Overlay resolution and fail-to-base](#overlay-resolution-and-fail-to-base).

> **Security invariant**: because every overlay failure falls back to it, the base `regions` in `EdgionPlugins` must be authored as the **safe default**.

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
        myRegion: us-east
        keyGet:
          - type: header
            name: X-Tenant-Id
        routeRules:
          - type: RouteByKeyConfMatch
            matchMap:
              tenant-a: us-east
              tenant-b: eu-west
          - type: RouteByHashRange
        hashCalc:
          algorithm: crc32
          modulo: 1000
        # Base region table — the safe default every overlay failure falls back to
        regions:
          - name: us-east
            hashRange: [0, 499]
            backendEndpoint: api.us-east.example.com
          - name: eu-west
            hashRange: [500, 999]
            backendEndpoint: api.eu-west.example.com
        # Optional: hot-swappable region table
        overrideRef:
          name: my-region-override
```

---

## Configuration Parameters

### Plugin config (`EdgionPlugins`)

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `myRegion` | String | `""` | This gateway instance's own region name. When a rule resolves to this region, the request passes through the gateway's own chain instead of jumping to a remote region |
| `keyGet` | Array | `[]` | Shared key extraction sources for `RouteByKeyConfMatch` and `RouteByHashRange`. The first non-empty extracted value wins |
| `hashKeyGet` | Array | none | Pre-computed hash sources, tried before computing a hash |
| `hashCalc` | Object | none | Hash computation config — see [Hash computation](#hash-computation) |
| `routeRules` | Array | `[]` | Ordered route rules, evaluated top to bottom, first match wins — see [Route rules](#route-rules) |
| `routeByKeyConfMatch` | Object | none | Cluster-level override for `RouteByKeyConfMatch`: `matchMap` (key → region) plus `bypassClusterKeys` (keys that ignore the override and fall back to the rule's own `matchMap`) |
| `regions` | Array | `[]` | **Base region table — the safe default.** Used directly when no overlay is configured, and on every overlay failure — see [Region definition](#region-definition) |
| `overrideRef` | Object | none | `{ name, namespace? }` reference to an `EdgionConfigData` of type `RegionRouteOverride`. Omitting `namespace` means the `EdgionPlugins` resource's own namespace. A cross-namespace reference requires a permitting `ReferenceGrant` or a target with `visibility: Cluster` |

`dye` is an **entry-level** field — a sibling of `type`/`config`/`enable`, not a member of `config`:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `dye` | Object | none | Traffic-tagging headers keyed on outcome (`on: [success, fallback]` — RegionRoute has no `reject` capability, see below). `request`/`response` entry lists of `{name, on, value, conditions}`; an omitted `value` carries the final region name (post-`failoverTo`) on `success`. See [Dye](./dye.md) for the full reference, and [Dye Headers](#dye-headers) below for the plugin-specific summary. |

The plugin performs no local field validation beyond the shared `dye` shape. Whether `overrideRef` resolves is checked by the Controller and surfaced on the `EdgionPlugins` resource's `ResolvedRefs` status condition; the region table itself is validated by the Gateway when the overlay is applied.

### Route rules

Evaluated **in order — first match wins**:

| Rule type | Behavior |
|-----------|----------|
| `RouteByDirectRegion` | Extracts a region name directly from the request via this rule's own `keyFrom` sources, in order; the first value matching a region in the effective table is used. Non-matching values fall through |
| `RouteByKeyConfMatch` | Extracts a key via the shared `keyGet` and looks it up in a `matchMap` (key → region). The cluster-level `routeByKeyConfMatch.matchMap` takes precedence when non-empty, unless the extracted key is listed in `bypassClusterKeys`, in which case the rule's own `matchMap` is used. On match, routes to the mapped region; otherwise falls through |
| `RouteByHashRange` | Consistent hash-range routing. Reads a pre-computed hash from `hashKeyGet` if present and valid (in `[0, modulo)`), otherwise computes it via `hashCalc`. This rule is **terminal** — it never falls through to later rules |

### Region definition

Each region (cluster) in `regions` defines:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | String | — | Cluster identifier (used in direct-region matching and failover). Must be non-empty and unique within the table |
| `hashRange` | `[Integer, Integer]` | — | Closed hash interval `[low, high]`; must satisfy `0 ≤ low ≤ high < modulo` |
| `backendEndpoint` | String | — | Target endpoint `host` or `host:port`. Only the upstream IP changes; the original SNI and request headers are preserved |
| `tls` | Boolean | `true` | Whether the upstream connection uses TLS (port defaults to `443` when `true`, `80` when `false`) |
| `failoverTo` | String | none | Redirect traffic for this cluster to the named cluster; must be a valid cluster in the same list and cannot be itself |

### Hash computation

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `algorithm` | Enum | `crc32` | Hash algorithm: `crc32` (fast, hardware-accelerated) or `sha256` |
| `modulo` | Integer | `1000` | Hash value domain `[0, modulo-1]`; every region's `hashRange[1]` must be `< modulo` |
| `writeBack` | Array | none | Targets to which the computed hash is written back (empty = no write-back) |

---

## Overlay resolution and fail-to-base

### The overlay resource

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: my-region-override
  namespace: production
spec:
  enable: true            # envelope kill-switch
  data:
    type: RegionRouteOverride
    config:
      # every field is optional; an omitted field keeps the base value
      regions:            # when present, WHOLE-REPLACES the base table (no element merge)
        - name: us-east
          hashRange: [0, 999]
          backendEndpoint: api.us-east.example.com
        - name: eu-west
          hashRange: [0, 0]
          backendEndpoint: api.eu-west.example.com
          failoverTo: us-east
```

| Overlay field | Effect |
|---------------|--------|
| `regions` | **Whole-replaces** the base region table. There is no element-wise merge — the table you supply is the table in effect |

`regions` is the only field with an effect. The `RegionRouteOverride` type also declares `enable` and `active`, but the Gateway consumes neither today — setting them changes nothing. To take an overlay out of service, set the **envelope** switch `spec.enable: false` on the `EdgionConfigData` (not `spec.data.config.enable`); the plugin then falls back to the base `regions`, as described below.

Routing **logic** (`routeRules`, `keyGet`, `hashCalc`, `routeByKeyConfMatch`, `myRegion`) is deliberately absent from the overlay. Logic lives only in `EdgionPlugins` and cannot be overridden remotely.

### Fail-to-base

The overlay is resolved on every request. All four failure paths fall back to the base `regions` — traffic continues, it is never rejected — and each records a `fallback` dye outcome with cause `overlay-fail-to-base` plus a Prometheus counter:

| Situation | Counter |
|-----------|---------|
| Referenced `EdgionConfigData` has `spec.enable: false` | `overlay_disabled` |
| Referenced `EdgionConfigData` does not exist | `overlay_missing` |
| It exists but `spec.data.type` is not `RegionRouteOverride` | `overlay_type_mismatch` |
| The merged region table fails validation (empty, empty name or endpoint, reversed `hashRange`, duplicate name) | `overlay_invalid` |

---

## Common Configuration Scenarios

### 1. Pin Specific Tenants to a Region (direct mapping)

Use a `RouteByKeyConfMatch` rule that maps a header value to a region, so named tenants always land in the same cluster:

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
        keyGet:
          - type: header
            name: X-Tenant-Id
        routeRules:
          - type: RouteByKeyConfMatch
            matchMap:
              tenant-a: us-east
              tenant-b: eu-west
        regions:
          - name: us-east
            hashRange: [0, 499]
            backendEndpoint: api.us-east.example.com
          - name: eu-west
            hashRange: [500, 999]
            backendEndpoint: api.eu-west.example.com
```

### 2. Consistent Hash-Range Distribution

Give each region a `hashRange` slice over `modulo` (e.g. `0–499` and `500–999` for a 50/50 split) and add `RouteByHashRange`. A request whose computed hash falls in a region's range is routed there. Because `RouteByHashRange` is terminal, place it last.

### 3. Region Failover

Set `failoverTo` on a region and traffic destined for that region is transparently redirected to the named healthy region — the HTTPRoute does not change.

Two ways to drive it:

- **Statically**, on a region in the `EdgionPlugins` base `regions`.
- **Dynamically**, by giving the plugin an `overrideRef` and writing `failoverTo` into the `RegionRouteOverride` EdgionConfigData. This is the path Center uses for multi-cluster failover, and it takes effect on the next request with no `EdgionPlugins` edit.

---

## Dye Headers

See [Dye](./dye.md) for the full reference: the `on` outcome vocabulary, the two `value` provenances, the template engine, the anti-spoof strip (runs even for `enable: false` and condition-skipped entries), header-name validation, and security guidance. This section covers only what is specific to RegionRoute.

`dye` is default-off: when omitted, the plugin sets no headers and strips nothing. When configured, it tags the request (or, on `response`-direction rules, the response) with operator-chosen HTTP headers:

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-region-route-plugin
  namespace: production
spec:
  requestPlugins:
    - type: RegionRoute
      dye:
        request:
          - name: X-Served-Region     # value omitted => final region name (on: [success] by default)
        response:
          - name: X-Edgion-Outcome
            on: [fallback]            # value omitted => region_route:fallback:overlay-fail-to-base
      config:
        regions:
          - name: us-east
            hashRange: [0, 999]
            backendEndpoint: api.us-east.example.com
        overrideRef:
          name: my-region-override
```

- `request` entries are set on the request forwarded to the upstream (or, for a local-region pass, on the request continuing through the gateway's own chain); `response` entries are set on the response returned to the client.
- An omitted `value` on `success` carries the **final region name** — that is the region *after* any `failoverTo` redirect is applied, not the originally-matched region.
- `success` dyeing applies whenever a rule (`RouteByDirectRegion`, `RouteByKeyConfMatch`, or the terminal `RouteByHashRange`) actually selects a region, including the **local-region pass** case — when the resolved region is this gateway's own region, no cross-region jump happens, but the request was still routed to a region, so it is dyed with that region's name.
- RegionRoute declares `success` and `fallback` — **not** `reject` — dye capabilities: its only failure-shaped branch is the overlay fail-to-base path (`RegionRouteOverride` resolution failing back to the base routing rules), recorded as `fallback` with cause `overlay-fail-to-base`. Dyeing does not happen when another plugin has already locked the upstream jump, or when no rule matches at all — neither is a recorded outcome today. `reject` is reserved for when the pre-existing `SetJumpOutcome::Rejected`-silently-proceeds bug is fixed (tracked separately; see the design record).

---

## FAQ

### Q1: Do I need an EdgionConfigData to use RegionRoute?

**A**: No. The `EdgionPlugins` config is complete on its own — routing logic and the base region table both live there. Add an `overrideRef` only when you want the region table to be swappable at runtime without editing `EdgionPlugins`, which is how Center drives multi-cluster failover.

### Q2: What is the order of rule evaluation?

**A**: Rules are evaluated top to bottom, first match wins. `RouteByDirectRegion` and `RouteByKeyConfMatch` fall through when they do not match, so put more specific rules first. `RouteByHashRange` is terminal and should be last.

### Q3: I updated the override but routing did not change. Why?

**A**: The overlay silently falls back to the base `regions` on any resolution problem — that is by design (fail-to-base, not fail-closed). Check, in order: the `EdgionPlugins` `ResolvedRefs` status condition (does the ref resolve at all, and is the cross-namespace reference permitted?), the referenced resource's `spec.enable` and its `spec.data.type`, and the gateway's plugin log, which records which of `overlay_disabled` / `overlay_missing` / `overlay_type_mismatch` / `overlay_invalid` applied.

### Q4: Can an override change the route rules?

**A**: No, and that is deliberate. `RegionRouteOverride` carries data only, and in practice only `regions` has any effect. Routing logic is exclusively in `EdgionPlugins`, so a remote writer can retarget traffic but cannot rewrite how traffic is classified.

---

## Related Documentation

- [Dynamic Upstream](./dynamic-upstream.md) — Request-driven upstream selection
- [Direct Endpoint](./direct-endpoint.md) — Bypass load balancing to a specific endpoint
- [Filters Overview](../overview.md) — All HTTPRoute filters
