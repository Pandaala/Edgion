# RealIp Plugin

> **🔌 Edgion extension**
> 
> RealIp is a real client IP extraction plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

## Overview

The RealIp plugin extracts the real client IP address from HTTP request headers, particularly useful when the gateway is deployed behind CDNs, load balancers, or other proxies.

This plugin implements the Nginx-style IP extraction algorithm, supporting trusted proxy configuration and recursive lookup.

## Features

- ✅ **Trusted proxy list** - Supports CIDR format trusted proxy configuration
- ✅ **Multiple header support** - Supports X-Forwarded-For, X-Real-IP, CF-Connecting-IP, etc.
- ✅ **Recursive lookup** - Nginx-style right-to-left traversal algorithm
- ✅ **IPv4/IPv6 support** - Full support for both IPv4 and IPv6 addresses
- ✅ **Performance optimized** - Uses pre-compiled IP Radix Tree for fast matching

## Configuration

### Basic Configuration

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: realip-proxies-1
  namespace: production
spec:
  visibility: Namespace
  data:
    type: IpList
    config:
      items:
      - name: private-ranges
        cidrs:
        - 10.0.0.0/8
        - 172.16.0.0/12
        - 192.168.0.0/16
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-basic
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIpRefs:
          - name: realip-proxies-1
            namespace: production
        realIpHeader: "X-Forwarded-For"
        recursive: true
```

### Configuration Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `trustedIpRefs` | Array\<EdgionConfigDataRef\> | — (required) | Nonempty references to trusted proxy IpLists |
| `realIpHeader` | String | `"X-Forwarded-For"` | Header name to extract the real IP from |
| `recursive` | Boolean | `true` | Whether to enable recursive lookup (Nginx-style) |
| `maxTrustedHops` | Integer | none (unlimited) | Cap on trusted-hop traversal when `recursive=true`. See [Security Trade-offs](#security-trade-offs-trustediprefs-and-maxtrustedhops) |

#### trustedIpRefs

A Kubernetes update rejected by API admission for required references, minimum size, target namespace or header constraints never reaches the Controller/Gateway; the previous admitted generation remains active. Invalid-generation replacement below refers to input that reaches typed publication (for example FS input or preserved mixed legacy keys), not an API-rejected update.

References are ordered `EdgionConfigData` IpLists. Put IP/CIDR collections in each IpList's `items` (IpGroups), then reference them by `name` and optional `namespace`. HTTP plugin references default to the actual owning plugin namespace. Global `spec.realIp` references require an explicit namespace and current `Cluster` visibility when ReferenceGrant validation is enabled; namespaced grants do not authorize this cluster owner. Disabling that validation preserves identity, IpList type and enabled checks.

Every referenced list must be available before trusting the effective downstream peer or any forwarded hop. A missing, disabled, denied or wrong-type list disables trust for this decision and uses the PROXY-effective peer, even when another list matches. Content updates, deletion, visibility changes and restoration affect subsequent decisions without copying CIDRs into the consumer. One captured view covers the peer and all hops. Invalid entries in an otherwise valid IpList are excluded by the central partial-good compiler; an empty/all-invalid matcher trusts no addresses.

| Field | Meaning |
| --- | --- |
| `name` | Required IpList resource name |
| `namespace` | HTTP: defaults to the owning namespace; global EGC: required |

Removed inline `trustedIps` and unknown fields are invalid even when null or empty. Invalid policies that reach typed publication replace the previous generation with a sealed plugin failure or disable global extraction. A clean generation restores the policy. Global EGC `Accepted` reports intrinsic structure only, never deployment-local list availability.

#### realIpHeader

Specifies which HTTP Header to extract the real IP from. Corresponds to Nginx's `real_ip_header` directive.

Common values:
- `X-Forwarded-For` - Standard proxy header (default)
- `X-Real-IP` - Common Nginx header
- `CF-Connecting-IP` - Cloudflare CDN
- `True-Client-IP` - Akamai CDN

The name must be a valid HTTP header field name (RFC 9110 `token` charset:
letters, digits and `!#$%&'*+-.^_`|~`) of at most 256 bytes. A typo such as
`"X Forwarded For"` can never match a parsed header at runtime, so without this
check real-IP extraction would be silently disabled and `RemoteIp` would stay
pinned to the trusted proxy address. An invalid name is therefore reported at
admission: the resource is stamped `Accepted=False` with `reason=Invalid`.

**Fail-closed on the plugin path.** For `EdgionPlugins`, an invalid RealIp
configuration seals the plugin chain closed — every request matching that
resource is answered with `500 plugin configuration error` rather than passing
through with extraction disabled. Fix the header name to restore traffic. The
global `EdgionGatewayConfig.spec.realIp` path is not sealed: the Gateway logs
an error and runs with real-IP extraction disabled, exactly as if `realIp` were
unset.

#### recursive

Enables recursive lookup mode. Corresponds to Nginx's `real_ip_recursive` directive.

- `true` (default): Traverses the IP list in the header from right to left, finding the first non-trusted IP
- `false`: Uses the rightmost parsed hop if untrusted; otherwise uses the effective downstream peer. The hop cap is ignored.

#### maxTrustedHops

Cap on trusted-proxy hops to traverse when `recursive=true` (optional). Mirrors Envoy's `xff_num_trusted_hops`.

- `null` (default): No cap — Nginx-compatible behavior, traverse right-to-left until the first non-trusted IP
- `0`: Stops at the first trusted parsed hop; it does not mean unlimited.
- `N` (>0): Skip at most `N` trusted IPs in the right-to-left walk; if all `N` are trusted, return the IP at position `N` even if trusted

**When to set it?**

`maxTrustedHops` is a **secondary defense** against the "`trustedIpRefs` scoped too broadly + attacker reachable from that range" scenario (see [Security Trade-offs](#security-trade-offs-trustediprefs-and-maxtrustedhops) below). Recommended for production: set it to your **actual proxy-chain depth** (e.g., CDN → LB → Gateway = `2`).

## Security Trade-offs: trustedIpRefs and maxTrustedHops

### Core Threat: XFF Depth Forgery Under Broad Trust CIDRs

With `recursive=true` and no hop cap, if `trustedIpRefs` is scoped too broadly (e.g., the entire `10.0.0.0/8`) and attackers can originate requests from within that range:

```text
Config: trustedIpRefs: [{name: internal-proxies}], recursive: true
Referenced IpList internal-proxies contains the broad CIDR 10.0.0.0/8.
Attacker at 10.0.0.1 sends:
  X-Forwarded-For: 1.2.3.4, 10.0.0.99, 10.0.0.50

Right-to-left traversal:
  10.0.0.50 → trusted → skip
  10.0.0.99 → trusted → skip
  1.2.3.4   → NOT trusted → return

Result: gateway resolves `RemoteIp = 1.2.3.4` (attacker-forged)
```

The attacker can then:
- Bypass IP allowlists (`ip_restriction` plugin) that key on `RemoteIp`
- Attribute rate-limit counters to arbitrary other users
- Pollute audit logs with forged source IPs

### Two Layers of Defense (configure both)

**Layer 1 (primary): Scope `trustedIpRefs` precisely**

The referenced IpLists **must contain only the actual proxy egress IPs** — do not use broad internal CIDRs for convenience:

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: realip-proxies-1
  namespace: default
spec:
  visibility: Namespace
  data:
    type: IpList
    config:
      items:
      - name: internal
        cidrs:
        - 10.0.0.0/8
---
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: realip-proxies-2
  namespace: default
spec:
  visibility: Namespace
  data:
    type: IpList
    config:
      items:
      - name: cloudflare
        cidrs:
        - 103.21.244.0/22
        - 103.22.200.0/22
        - 173.245.48.0/20
      - name: internal-lb
        description: Internal LB egress (single address)
        cidrs:
        - 10.1.0.10
---
# ❌ Bad: too broad — any internal request is treated as a trusted proxy
trustedIpRefs:
  - name: realip-proxies-1
    namespace: default
# ✅ Good: only the actual proxies (Cloudflare origin ranges, specific internal LB)
trustedIpRefs:
  - name: realip-proxies-2
    namespace: default
```

**Layer 2 (secondary): Set `maxTrustedHops` as a hard ceiling**

Even if Layer 1 is misconfigured, set `maxTrustedHops = actual proxy-chain depth` to limit how far an attacker can push the result with a forged trusted-looking prefix:

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: realip-proxies-1
  namespace: default
spec:
  visibility: Namespace
  data:
    type: IpList
    config:
      items:
      - name: cloudflare
        cidrs:
        - 103.21.244.0/22
      - name: internal-lb
        cidrs:
        - 10.1.0.10
---
trustedIpRefs:
  - name: realip-proxies-1
    namespace: default
maxTrustedHops: 2         # Only trust 2 hops; attacker cannot push result leftward
                          # by padding more "trusted" IPs
```

### When `maxTrustedHops` Can Be Omitted

- `recursive: false`: has no effect (nonrecursive uses an untrusted rightmost hop, otherwise the peer)
- the referenced IpLists contain only a handful of specific IPs / small CIDRs and is audited
- Proxy chain is dynamic (e.g., service mesh scaling nodes) and a fixed hop count is not known — rely on Layer 1 only

### Comparison with Nginx / Envoy

| Behavior | Edgion | Nginx | Envoy |
|----------|--------|-------|-------|
| Recursive traversal | `recursive: true` | `real_ip_recursive on` | — (implicit) |
| Hop cap | `maxTrustedHops` (opt-in) | ❌ None | `xff_num_trusted_hops` (required) |
| Default | Unlimited (Nginx-compatible) | Unlimited | 0 |

Edgion follows the Nginx style by default (unlimited, backward-compatible) while providing `maxTrustedHops` for Envoy-familiar operators and security-sensitive deployments.

## How It Works

### Algorithm Flow

```text
1. Check if downstream_peer_ip is in trustedIpRefs
   ├─ No → Keep downstream_peer_ip as RemoteIp
   └─ Yes → Continue to step 2

2. Extract IP list from realIpHeader
   e.g.: X-Forwarded-For: "203.0.113.1, 198.51.100.2, 192.168.1.1"

3. Recursive lookup (if recursive=true)
   Traverse from right to left:
   ├─ 192.168.1.1 → In trustedIpRefs ✓ Continue
   ├─ 198.51.100.2 → In trustedIpRefs ✓ Continue
   └─ 203.0.113.1 → Not in trustedIpRefs ✗ This is the real IP!

4. Update ctx.request_info.remote_ip (via set_remote_ip method)
```

### Example Scenarios

#### Scenario 1: Typical CDN + Load Balancer

```text
Real client: 203.0.113.1
    ↓
CDN (198.51.100.2, trusted)
    ↓ X-Forwarded-For: 203.0.113.1
Load Balancer (192.168.1.1, trusted)
    ↓ X-Forwarded-For: 203.0.113.1, 198.51.100.2
Edgion Gateway (downstream_peer_ip: 192.168.1.1)
```

**Configuration:**
```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: realip-proxies-1
  namespace: default
spec:
  visibility: Namespace
  data:
    type: IpList
    config:
      items:
      - name: load-balancer
        cidrs:
        - 192.168.0.0/16
      - name: cdn
        cidrs:
        - 198.51.100.0/24
---
trustedIpRefs:
  - name: realip-proxies-1
    namespace: default
realIpHeader: "X-Forwarded-For"
recursive: true
```

**Result:** `remote_ip = 203.0.113.1`

#### Scenario 2: Cloudflare CDN

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: realip-proxies-1
  namespace: default
spec:
  visibility: Namespace
  data:
    type: IpList
    config:
      items:
      - name: cloudflare
        cidrs:
        - 173.245.48.0/20
        - 103.21.244.0/22
        - 103.22.200.0/22
---
trustedIpRefs:
  - name: realip-proxies-1
    namespace: default
realIpHeader: "CF-Connecting-IP"
recursive: false
```

## Usage Examples

### Example 1: Basic Configuration

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: realip-proxies-1
  namespace: production
spec:
  visibility: Namespace
  data:
    type: IpList
    config:
      items:
      - name: private-ranges
        cidrs:
        - 10.0.0.0/8
        - 172.16.0.0/12
        - 192.168.0.0/16
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-basic
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIpRefs:
          - name: realip-proxies-1
            namespace: production
```

### Example 2: Cloudflare Configuration

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: realip-proxies-1
  namespace: production
spec:
  visibility: Namespace
  data:
    type: IpList
    config:
      items:
      - name: cloudflare
        cidrs:
        - 173.245.48.0/20
        - 103.21.244.0/22
        - 103.22.200.0/22
        - 103.31.4.0/22
        - 141.101.64.0/18
        - 108.162.192.0/18
        - 190.93.240.0/20
        - 188.114.96.0/20
        - 197.234.240.0/22
        - 198.41.128.0/17
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-cloudflare
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIpRefs:
          - name: realip-proxies-1
            namespace: production
        realIpHeader: "CF-Connecting-IP"
        recursive: false
```

### Example 3: Multi-Tier Proxy

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: realip-proxies-1
  namespace: production
spec:
  visibility: Namespace
  data:
    type: IpList
    config:
      items:
      - name: internal
        description: Internal proxy and private networks
        cidrs:
        - 10.0.0.0/8
        - 172.16.0.0/12
        - 192.168.0.0/16
      - name: cdn
        cidrs:
        - 198.51.100.0/24
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-multi-tier
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIpRefs:
          - name: realip-proxies-1
            namespace: production
        realIpHeader: "X-Forwarded-For"
        recursive: true
```

## Comparison with Other Gateways

| Feature | Edgion RealIp | Nginx | APISIX | Kong |
|---------|---------------|-------|--------|------|
| Trusted proxy CIDR | ✅ | ✅ `set_real_ip_from` | ✅ `trusted_addresses` | ⚠️ Simple config |
| Custom header | ✅ | ✅ `real_ip_header` | ✅ `source` | ✅ |
| Recursive lookup | ✅ | ✅ `real_ip_recursive` | ✅ `recursive` | ❌ |
| Route-level config | ✅ | ❌ Global only | ✅ | ❌ |

## Notes

1. **Security**: Only trust proxy IPs you control; do not trust overly broad CIDR ranges
2. **Performance**: IP matching uses a Radix Tree with minimal performance overhead
3. **Order**: This plugin should execute before other plugins that use `remote_ip` (such as rate limiting, IP restriction)
4. **Global config**: If both global `realIp` and plugin-level `RealIp` are configured, the plugin configuration overrides the global one
5. **Real-time updates**: The plugin directly modifies `ctx.request_info.remote_ip`; subsequent plugins and access logs will use the updated value

## FAQ

### Q: Why distinguish between DirectPeerIp and RemoteIp?

A: 
- `DirectPeerIp`: The physical source IP of the transport connection (usually the load balancer)
- `RemoteIp`: The client identity resolved from a trusted PROXY/RealIp boundary (used for business logic)

### Q: How can I verify the configuration is working?

A: Check the `remote_ip` field in request logs, or check the configured RemoteIp header in the backend service (default: `X-Real-IP`).

### Q: Can I trust multiple CDNs simultaneously?

A: Yes, put each CDN in an IpGroup inside one shared IpList, or reference multiple IpLists.

### Q: Does it support dynamic updates?

A: Yes, after updating the EdgionPlugins resource, the configuration is automatically hot-reloaded.

## Related Documentation

- [IP Restriction Plugin](../http-route/filters/edgion-plugins/ip-restriction.md) - IP allow/deny lists (commonly used with RealIp)
- [HTTPRoute Overview](../http-route/overview.md) - HTTPRoute configuration overview
- [Nginx ngx_http_realip_module](https://nginx.org/en/docs/http/ngx_http_realip_module.html) - Nginx real IP module reference
- [Cloudflare IP Ranges](https://www.cloudflare.com/ips/) - Cloudflare CDN IP ranges
