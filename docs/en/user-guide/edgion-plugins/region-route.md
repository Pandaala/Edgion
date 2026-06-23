# RegionRoute Plugin

> **Edgion extension**
>
> RegionRoute is a multi-region traffic routing plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

## Overview

RegionRoute routes traffic across multiple regions using ordered route rules. It is designed for multi-region
active-active deployments where the same service runs in two or more regions and you need deterministic,
sticky routing per account, user, or any other key.

**Routing is governed by `routeRules` — evaluated in order, first match wins; all miss → pass through.**

Three rule types are available:

| Rule | Behaviour |
|------|-----------|
| `RouteByDirectRegion` | Extract a region name from the request and route there directly |
| `RouteByKeyConfMatch` | Match extracted key against a fixed key→region map |
| `RouteByHashRange` | Consistent hash-range routing (0 to modulo-1); **terminal** — stops rule evaluation |

**Key behaviors:**
- Only the upstream **IP** changes (via DNS resolution of `backendEndpoint`). The original request's **Host
  header**, **SNI**, path, and body are all preserved unchanged.
- Each region occupies a slice of the `[0, modulo-1]` range; slices are non-overlapping and can have gaps.
- `myRegion`: when the routing target equals this gateway's own region, the plugin passes through without any
  cross-region jump.
- `failoverTo`: static redirect on a per-region basis — traffic routed to that region is redirected to another.
- Empty `routeRules` → plugin is a no-op.

## Architecture: Base + Overlay Model

RegionRoute configuration follows the **EdgionConfigData overlay model**:

| Layer | Resource | Owner | Description |
|-------|----------|-------|-------------|
| Base | `EdgionPlugins` RegionRoute config | git/operator | Logic + default region table. Always present, complete, and safe. |
| Overlay (optional) | `RegionRouteOverride` `EdgionConfigData` | operator / Center | Hot-swappable region table; replaces the base table whole when applied. |

The plugin base config (`EdgionPlugins`) carries all routing logic **and** a default `regions` table. An
optional `overrideRef` points to an `EdgionConfigData` object carrying a `RegionRouteOverride` variant for dynamic,
hot-swappable region table updates.

**Fail-to-base:** if the overlay is absent, invalid, or the wrong type, the gateway uses the default `regions`
from the base config — traffic continues without interruption.

## Configuration Parameters

### EdgionPlugins RegionRouteConfig (base)

All routing logic lives here. The `overrideRef` field is optional.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `myRegion` | String | `""` | This gateway's own region; pass through locally when routing target equals this |
| `regions` | Array | `[]` | Default region definitions (safe default table, used when no overlay or overlay fails) |
| `keyGet` | Array | `[]` | Shared key extraction config |
| `hashKeyGet` | Array | none | Pre-computed hash sources (read before computing in RouteByHashRange) |
| `hashCalc` | Object | none | Hash computation config; absent = RouteByHashRange is passthrough-only |
| `routeRules` | Array | `[]` | Ordered route rules list; empty = plugin no-op |
| `routeByKeyConfMatch` | Object | none | RouteByKeyConfMatch matchMap config |
| `overrideRef` | Object | none | Optional reference to a `RegionRouteOverride` EdgionConfigData |

### `regions[]` (RegionDef) Sub-fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | String | required | Region identifier |
| `hashRange` | Array\<Integer\> | required | Hash range `[low, high]`, closed interval |
| `backendEndpoint` | String | required | Backend endpoint, format `host` or `host:port` |
| `tls` | Boolean | `true` | Whether upstream connection uses TLS |
| `failoverTo` | String | none | Static failover target region name |

### `hashCalc` Sub-fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `algorithm` | Enum | `crc32` | Hash algorithm: `crc32` or `sha256` |
| `modulo` | Integer | `1000` | Hash value domain `[0, modulo-1]`, must be >= 2 |
| `writeBack` | Array | `[]` | Targets to write the computed hash to |

### EdgionConfigData RegionRouteOverride Config (overlay)

All fields are optional — only the fields present in the overlay replace the base value.

| Parameter | Type | Description |
|-----------|------|-------------|
| `regions` | Array | Complete replacement of the region table (whole-replace, no merge) |

---

## Quick Start

### Step 1: Create EdgionPlugins with base config

All logic and a safe default region table live in `EdgionPlugins`:

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-app-plugins
  namespace: production
spec:
  requestPlugins:
    - type: RegionRoute
      config:
        myRegion: virginia

        # Default region table (used when overlay is absent or fails)
        regions:
          - name: virginia
            hashRange: [0, 599]
            backendEndpoint: virginia-internal.example.com:8443
            tls: true
          - name: ohio
            hashRange: [600, 999]
            backendEndpoint: ohio-internal.example.com:8443
            tls: true

        keyGet:
          - type: header
            name: X-Account-Id
          - type: cookie
            name: account_id

        hashCalc:
          algorithm: crc32
          modulo: 1000
          writeBack:
            - type: responseHeader
              name: X-Region-Hash

        routeRules:
          - type: RouteByDirectRegion
            keyFrom:
              - type: header
                name: X-Direct-Region
          - type: RouteByKeyConfMatch
          - type: RouteByHashRange

        routeByKeyConfMatch:
          matchMap:
            "vip-001": virginia
            "enterprise-xyz": ohio

        # Optional: point to a hot-swappable overlay for dynamic region updates
        overrideRef:
          name: region-overlay
          namespace: edgion-system
```

### Step 2 (Optional): Create a RegionRouteOverride EdgionConfigData

Create an EdgionConfigData with `type: RegionRouteOverride` to hot-swap the region table without a config
redeploy:

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: region-overlay
  namespace: edgion-system
  labels:
    edgion.io/managed-by: operator
spec:
  enable: true
  data:
    type: RegionRouteOverride
    config:
      regions:                # whole-replace of the base region table
        - name: virginia
          hashRange: [0, 499]
          backendEndpoint: virginia-v2.example.com:8443
          tls: true
        - name: ohio
          hashRange: [500, 999]
          backendEndpoint: ohio-v2.example.com:8443
          tls: true
          failoverTo: virginia
```

Updating this object hot-swaps the region table on all gateways referencing it, without touching the
`EdgionPlugins` config. Deleting it causes the gateway to fall back to the base region table immediately.

---

## Region Hash Ranges

`hashCalc.modulo` (default **1000**) defines the hash domain `[0, modulo-1]`. All region `hashRange`
values must satisfy `hashRange[1] < modulo`.

| Intent | virginia range | ohio range |
|--------|---------------|-----------|
| 50 / 50 | `[0, 499]` | `[500, 999]` |
| 60 / 40 | `[0, 599]` | `[600, 999]` |
| 80 / 20 | `[0, 799]` | `[800, 999]` |
| Canary (ohio=5%) | `[0, 949]` | `[950, 999]` |

Gaps are allowed. Requests whose hash falls in a gap pass through to the normal upstream.

## RouteByHashRange Detail

1. Read from `hashKeyGet` — if value parses as integer in `[0, modulo)`, use it directly.
2. Compute via `hashCalc` — extract key from shared `keyGet`, compute `crc32(key) % modulo`.
3. Write computed hash to `hashCalc.writeBack` targets.

`RouteByHashRange` is **terminal**: when it returns pass-through, later rules are not evaluated.

Any backend service can pre-compute the hash and store it in a cookie for subsequent requests:

```python
import zlib
region_hash = zlib.crc32(account_id.encode()) % 1000  # modulo must match config
# Set-Cookie: x_region_hash=<region_hash>
```

## Access Log

The plugin writes entries to the request's plugin log:

| Entry | Meaning |
|-------|---------|
| `direct=ohio(from:hdr:X-Direct-Region)` | Routed via direct override |
| `direct unknown 'foo', fallthrough` | Unknown region name, fell through |
| `match=hdr:X-Account-Id→virginia` | Key matched in matchMap |
| `match=hdr:X-Account-Id no conf` | Key not in matchMap, fell through |
| `hash_pass=342 from:cookie:x_region_hash` | Used pre-computed hash from cookie |
| `key=hdr:X-Account-Id hash=342` | Computed hash from header key |
| `redirect ohio→virginia` | Static failoverTo redirect applied |
| `local, pass` | Target equals myRegion — passed through locally |
| `err: failover to invalid` | failoverTo target misconfigured |
| `→virginia 127.0.0.1:30001` | Jump target (region, host:port) |
| `key missing, pass` | No key found, passed through |
| `overlay_missing` | overrideRef set but EdgionConfigData not found; using base |
| `overlay_type_mismatch` | EdgionConfigData type does not match RegionRouteOverride; using base |

## myRegion and failoverTo

### myRegion — Local Pass-Through

When a gateway instance is deployed inside one of the regions, set `myRegion` to that region's name. If
routing determines the target region is the local one, the plugin skips `ExternalJumpPreset` and passes
through to the local upstream — no cross-region network hop.

### failoverTo — Static Region Redirect

Add `failoverTo` to any `RegionDef` to statically redirect traffic away from that region. Works in both
the base `regions` table and in a `RegionRouteOverride` overlay. Typical use: mark a region
out-of-service; all traffic redirects to another region.

Constraints: target must be a valid region name; no chaining; no self-reference or cycles (silently
ignored with `err: failover to invalid` in access log).

## Notes

- `backendEndpoint` is DNS-resolved on every request (Pingora `ExternalJumpPreset`). Use a stable
  internal DNS name backed by your region's load balancer.
- The plugin does **not** change the `Host` header or SNI.
- Setting `routeRules: []` turns the plugin into a no-op — useful for staged rollout.
- The `RegionRouteOverride` EdgionConfigData can be in any namespace (specified via `overrideRef.namespace`).

## Related Documentation

- [HTTPRoute Overview](../http-route/overview.md)
- [Load Balancing Algorithms](../http-route/lb-algorithms.md)
