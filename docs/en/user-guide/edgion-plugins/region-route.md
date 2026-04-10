# RegionRoute Plugin

## Overview

RegionRoute routes traffic across multiple regions using a consistent hash-range model (0–1000). It is designed for multi-region active-active deployments where the same service runs in two or more regions and you need deterministic, sticky routing per account, user, or any other key.

**Routing priority (highest to lowest):**
1. `direct_region` — explicit override from request header/cookie (optional)
2. `region_key_hash` — pre-computed 0–1000 integer from header/cookie (skip hash)
3. `region_key` — extract key, compute SHA-256 hash mod 1001, map to region range

When `region_key` is empty or the key is absent from the request, the plugin passes through to the normal upstream without modification.

**Key behaviors:**
- Only the upstream **IP** changes (via DNS resolution of `backend_endpoint`). The original request's **Host header**, **SNI**, path, and body are all preserved unchanged.
- Each region occupies a slice of the 0–1000 range; slices are non-overlapping and can have gaps.
- Auto failover (optional) probes each region's DNS health; if a region has no IPs, traffic falls over to the healthiest alternative.

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
        # Region definitions — hash range [low, high], closed interval, 0–1000
        regions:
          - name: virginia
            hashRange: [0, 600]          # 60% of traffic
            backendEndpoint: virginia-internal.example.com:8443
            tls: true
          - name: ohio
            hashRange: [601, 1000]       # 40% of traffic
            backendEndpoint: ohio-internal.example.com:8443
            tls: true

        # Optional: service can force a specific region via this header/cookie
        directRegion:
          keyFrom:
            - type: header
              name: X-Direct-Region      # value must match a region name

        # Optional: pre-computed 0–1000 integer (avoids re-hashing on every request)
        regionKeyHash:
          keyFrom:
            - type: cookie
              name: x_region_hash        # e.g. Set-Cookie: x_region_hash=342

        # Hash key — tried in order, first non-empty value wins
        # Empty list = plugin is a no-op
        regionKey:
          - type: header
            name: X-Account-Id
          - type: cookie
            name: account_id
          # - type: clientIp            # uncomment for IP-based fallback

        # Optional: auto failover via DNS probe
        autoFailover:
          enabled: true
          healthyThreshold: 0.5         # trigger if DNS returns no IPs
          probeIntervalMs: 5000         # probe every 5 s
          notifyHeader: X-Failover-Region   # written to response (debugging)
          onAllRegionsDown: pass        # pass | reject (503)
```

## Region Hash Ranges

The 0–1000 range gives you explicit, human-readable control over traffic split:

| Intent | virginia range | ohio range |
|--------|---------------|-----------|
| 50 / 50 | `[0, 500]` | `[501, 1000]` |
| 60 / 40 | `[0, 600]` | `[601, 1000]` |
| 80 / 20 | `[0, 800]` | `[801, 1000]` |
| Canary (ohio=5%) | `[0, 950]` | `[951, 1000]` |

Gaps are allowed. Requests whose hash falls in a gap pass through to the normal upstream.

## Pre-computed Hash (regionKeyHash)

Any backend service can compute the hash once and store it in a cookie. The gateway reads it directly on subsequent requests, skipping SHA-256:

```python
# Python example — same algorithm as the gateway
import xxhash
# Gateway uses SHA-256: sha256(key)[0:8] as u64 % 1001
import hashlib, struct
digest = hashlib.sha256(account_id.encode()).digest()
h = struct.unpack(">Q", digest[:8])[0]
region_hash = h % 1001
# Set-Cookie: x_region_hash=<region_hash>
```

## Direct Region Override

Services can force a request to a specific region by setting `X-Direct-Region`:

```http
GET /api/data HTTP/1.1
Host: api.example.com
X-Account-Id: acct-123
X-Direct-Region: ohio
```

If the named region is unhealthy, the plugin falls through to auto failover (if enabled) rather than routing to an unhealthy backend.

## Access Log

The plugin writes entries to the request's plugin log:

| Entry | Meaning |
|-------|---------|
| `direct=ohio(from:hdr(X-Direct-Region));` | Routed via direct override from header |
| `hash_pass=342 from:cookie(x_region_hash);` | Used pre-computed hash from cookie |
| `key=hdr(X-Account-Id) hash=342;` | Computed hash from header key |
| `→virginia virginia-internal.example.com:8443 sni=api.example.com;` | Jump target |
| `failover→ohio (100%);` | Failed over to ohio |
| `region_key missing, pass;` | No key found, passed through |

## Notes

- `backend_endpoint` is DNS-resolved on every request (Pingora's existing `ExternalJumpPreset` mechanism). Use a stable internal DNS name backed by your region's load balancer.
- The plugin does **not** change the `Host` header or SNI — if your upstream verifies SNI, ensure the backend certificate covers the original hostname.
- Auto failover is based on DNS liveness only (does a DNS lookup return IPs?). It does not perform application-level health checks.
- Setting `regionKey: []` (empty list) turns the plugin into a no-op — useful for staged rollout.
