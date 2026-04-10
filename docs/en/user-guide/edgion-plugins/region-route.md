# RegionRoute Plugin

## Overview

RegionRoute routes traffic across multiple regions using ordered route rules. It is designed for multi-region active-active deployments where the same service runs in two or more regions and you need deterministic, sticky routing per account, user, or any other key.

**Routing is governed by `routeRules` — evaluated in order, first match wins, all miss → pass through.**

Three rule types are available:

| Rule | Behaviour |
|------|-----------|
| `RouteByDirectRegion` | Extract a region name from the request and route there directly |
| `RouteByKeyConfMatch` | Match extracted key against a fixed key→region map |
| `RouteByHashRange` | Consistent hash-range routing (0 to modulo-1); **terminal** — stops rule evaluation |

**Key behaviors:**
- Only the upstream **IP** changes (via DNS resolution of `backendEndpoint`). The original request's **Host header**, **SNI**, path, and body are all preserved unchanged.
- Each region occupies a slice of the `[0, modulo-1]` range; slices are non-overlapping and can have gaps.
- `myRegion`: when the routing target equals this gateway's own region, the plugin passes through without any cross-region jump (traffic is already local).
- `failoverTo`: static redirect on a per-region basis — traffic routed to that region is redirected to another region before the health check.
- Auto failover (optional) probes each region's DNS health; if a region has no IPs, traffic falls over to the healthiest alternative.
- Empty `routeRules` → plugin is a no-op.

## Configuration

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-region-route
  namespace: default
spec:
  requestPlugins:
    - type: RegionRoute
      config:
        # Region definitions — hashRange [low, high], closed interval
        regions:
          - name: virginia
            hashRange: [0, 599]           # 60% of traffic (600 of 1000 slots)
            backendEndpoint: virginia-internal.example.com:8443
            tls: true
            # failoverTo: ohio            # optional: redirect virginia traffic to ohio
          - name: ohio
            hashRange: [600, 999]         # 40% of traffic (400 of 1000 slots)
            backendEndpoint: ohio-internal.example.com:8443
            tls: true
            # failoverTo: virginia        # optional: redirect ohio traffic to virginia

        # Optional: this gateway's own region — if routing targets this region,
        # pass through without ExternalJumpPreset (traffic is already local).
        # myRegion: virginia

        # Shared key extraction for RouteByKeyConfMatch and RouteByHashRange
        keyGet:
          - type: header
            name: X-Account-Id
          - type: cookie
            name: account_id

        # Pre-computed hash sources for RouteByHashRange (read before computing)
        hashKeyGet:
          - type: cookie
            name: x_region_hash

        # Hash computation — if absent, RouteByHashRange is passthrough-only
        hashCalc:
          algorithm: crc32     # crc32 (default) | sha256
          modulo: 1000         # 1000 slots [0, 999]; hashRange[1] must be < modulo
          writeBack:           # write computed hash to these targets
            - type: responseHeader
              name: X-Region-Hash

        # Ordered route rules — first match wins
        routeRules:
          - type: RouteByDirectRegion
            keyFrom:
              - type: header
                name: X-Direct-Region

          - type: RouteByKeyConfMatch
            matchMap:
              "vip-001": virginia
              "enterprise-xyz": ohio

          - type: RouteByHashRange   # terminal rule, no per-rule config

        # Optional: auto failover via DNS probe
        autoFailover:
          enabled: true
          healthyThreshold: 0.5          # trigger if DNS returns no IPs
          probeIntervalMs: 5000          # probe every 5 s
          notifyHeader: X-Failover-Region    # written to response (debugging)
          onAllRegionsDown: pass         # pass | reject (503)
```

## Region Hash Ranges

`hashCalc.modulo` (default **1000**) defines the hash value domain `[0, modulo-1]` — exactly 1000 slots by default (0..=999). All region `hashRange` values must satisfy `hashRange[1] < modulo`.

| Intent | virginia range | ohio range |
|--------|---------------|-----------|
| 50 / 50 | `[0, 499]` | `[500, 999]` |
| 60 / 40 | `[0, 599]` | `[600, 999]` |
| 80 / 20 | `[0, 799]` | `[800, 999]` |
| Canary (ohio=5%) | `[0, 949]` | `[950, 999]` |

Gaps are allowed. Requests whose hash falls in a gap pass through to the normal upstream.

## RouteByHashRange Detail

`RouteByHashRange` first tries `hashKeyGet` (pre-computed passthrough), then falls back to `hashCalc` computation:

1. Read from `hashKeyGet` — if value parses as integer in `[0, modulo)`, use it directly (log: `hash_pass=N from:SRC`).
2. Compute via `hashCalc` — extract key from shared `keyGet`, compute `crc32(key) % modulo` (log: `key=SRC hash=N`).
3. Write computed hash to `hashCalc.writeBack` targets.

`RouteByHashRange` is **terminal**: when it returns pass-through (no key, no hashCalc, or hash in a gap), rule evaluation stops and the request passes through — later rules are not tried.

Any backend service can pre-compute the hash and store it in a cookie for subsequent requests:

```python
import zlib
region_hash = zlib.crc32(account_id.encode()) % 1000  # modulo must match config
# Set-Cookie: x_region_hash=<region_hash>
```

## Access Log

The plugin writes entries to the request's plugin log (100-byte limit — entries are short):

| Entry | Meaning |
|-------|---------|
| `direct=ohio(from:hdr:X-Direct-Region);` | Routed via direct override |
| `direct unknown 'foo', fallthrough;` | Unknown region name, fell through to next rule |
| `match=hdr:X-Account-Id→virginia;` | Key matched in matchMap, routed to virginia |
| `match=hdr:X-Account-Id no conf;` | Key not in matchMap, fell through |
| `hash_pass=342 from:cookie:x_region_hash;` | Used pre-computed hash from cookie |
| `key=hdr:X-Account-Id hash=342;` | Computed hash from header key |
| `redirect ohio→virginia;` | Static failoverTo redirect applied |
| `local, pass;` | Target equals myRegion — passed through locally |
| `err:failoverTo invalid;` | failoverTo target is misconfigured (not found/self-ref/cycle) |
| `→virginia 127.0.0.1:30001;` | Jump target (region, host:port) |
| `failover→ohio (100%);` | Failed over to ohio via autoFailover |
| `key missing, pass;` | No key found, passed through |

## myRegion and failoverTo

### myRegion — Local Pass-Through

When a gateway instance is deployed inside one of the regions, set `myRegion` to that region's name. If routing determines the target region is the local one, the plugin skips `ExternalJumpPreset` and passes through to the local upstream directly — no cross-region network hop.

```yaml
myRegion: virginia    # this gateway is deployed in virginia
```

- Empty string (default) = feature disabled; all routing always sets `ExternalJumpPreset`.
- If the value doesn't match any region name, the feature is silently disabled at runtime.

### failoverTo — Static Region Redirect

Add `failoverTo` to any `RegionDef` to statically redirect traffic away from that region. Applied after the routing rule determines the target, before the `autoFailover` health check.

```yaml
regions:
  - name: ohio
    hashRange: [600, 999]
    backendEndpoint: ohio-internal.example.com:8443
    failoverTo: virginia     # all ohio-bound traffic → virginia instead
```

Typical use cases:
- **Manual failover**: operator marks a region as out-of-service; all traffic redirects to another region.
- **Combined with myRegion**: if the redirect target equals `myRegion`, redirected traffic also passes through locally.

Constraints:
- Target must be a valid region name (or the redirect is silently ignored — `err:failoverTo invalid;` in access log).
- No chaining: only one level of redirect is applied.
- No self-reference or cycles (silently ignored at runtime with `err:failoverTo invalid;` logged).

## Notes

- `backendEndpoint` is DNS-resolved on every request (Pingora's existing `ExternalJumpPreset` mechanism). Use a stable internal DNS name backed by your region's load balancer.
- The plugin does **not** change the `Host` header or SNI — if your upstream verifies SNI, ensure the backend certificate covers the original hostname.
- Auto failover is based on DNS liveness only (does a DNS lookup return IPs?). It does not perform application-level health checks.
- Setting `routeRules: []` (empty list, or omitting it) turns the plugin into a no-op — useful for staged rollout.
