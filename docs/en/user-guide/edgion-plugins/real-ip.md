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
kind: EdgionPlugins
metadata:
  name: real-ip-basic
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
          - name: private-ranges
            cidrs:
              - "10.0.0.0/8"
              - "172.16.0.0/12"
              - "192.168.0.0/16"
        realIpHeader: "X-Forwarded-For"
        recursive: true
```

### Configuration Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `trustedIps` | Array\<IpGroup\> | — (required) | List of trusted proxy IP groups |
| `realIpHeader` | String | `"X-Forwarded-For"` | Header name to extract the real IP from |
| `recursive` | Boolean | `true` | Whether to enable recursive lookup (Nginx-style) |
| `maxTrustedHops` | Integer | none (unlimited) | Cap on trusted-hop traversal when `recursive=true`. See [Security Trade-offs](#security-trade-offs-trustedips-and-maxtrustedhops) |

#### trustedIps

List of trusted proxy IP groups. Corresponds to Nginx's `set_real_ip_from` directive.

Each entry is an `IpGroup` object:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | yes | Group name; ≤63 chars, charset `[a-zA-Z0-9._-]`, must start/end alphanumeric, unique within the list |
| `description` | String | no | Optional human-readable description of the group |
| `cidrs` | Array\<String\> | yes | Non-empty list of IP addresses or CIDR ranges |

Each `cidrs` entry supports:

- Single IP addresses: `"192.168.1.1"`
- CIDR ranges: `"10.0.0.0/8"`
- IPv6: `"2001:db8::/32"`

#### realIpHeader

Specifies which HTTP Header to extract the real IP from. Corresponds to Nginx's `real_ip_header` directive.

Common values:
- `X-Forwarded-For` - Standard proxy header (default)
- `X-Real-IP` - Common Nginx header
- `CF-Connecting-IP` - Cloudflare CDN
- `True-Client-IP` - Akamai CDN

#### recursive

Enables recursive lookup mode. Corresponds to Nginx's `real_ip_recursive` directive.

- `true` (default): Traverses the IP list in the header from right to left, finding the first non-trusted IP
- `false`: Uses the rightmost IP in the header (the last proxy)

#### maxTrustedHops

Cap on trusted-proxy hops to traverse when `recursive=true` (optional). Mirrors Envoy's `xff_num_trusted_hops`.

- `null` (default): No cap — Nginx-compatible behavior, traverse right-to-left until the first non-trusted IP
- `N` (>0): Skip at most `N` trusted IPs in the right-to-left walk; if all `N` are trusted, return the IP at position `N` even if trusted

**When to set it?**

`maxTrustedHops` is a **secondary defense** against the "`trustedIps` scoped too broadly + attacker reachable from that range" scenario (see [Security Trade-offs](#security-trade-offs-trustedips-and-maxtrustedhops) below). Recommended for production: set it to your **actual proxy-chain depth** (e.g., CDN → LB → Gateway = `2`).

## Security Trade-offs: trustedIps and maxTrustedHops

### Core Threat: XFF Depth Forgery Under Broad Trust CIDRs

With `recursive=true` and no hop cap, if `trustedIps` is scoped too broadly (e.g., the entire `10.0.0.0/8`) and attackers can originate requests from within that range:

```text
Config:  trustedIps: ["10.0.0.0/8"], recursive: true
Attacker at 10.0.0.1 sends:
  X-Forwarded-For: 1.2.3.4, 10.0.0.99, 10.0.0.50

Right-to-left traversal:
  10.0.0.50 → trusted → skip
  10.0.0.99 → trusted → skip
  1.2.3.4   → NOT trusted → return

Result: gateway thinks client_ip = 1.2.3.4 (attacker-forged)
```

The attacker can then:
- Bypass IP allowlists (`ip_restriction` plugin) that key on `client_ip`
- Attribute rate-limit counters to arbitrary other users
- Pollute audit logs with forged source IPs

### Two Layers of Defense (configure both)

**Layer 1 (primary): Scope `trustedIps` precisely**

`trustedIps` **must list only the actual proxy egress IPs** — do not use broad internal CIDRs for convenience:

```yaml
# ❌ Bad: too broad — any internal request is treated as a trusted proxy
trustedIps:
  - name: internal
    cidrs:
      - "10.0.0.0/8"

# ✅ Good: only the actual proxies (Cloudflare origin ranges, specific internal LB)
trustedIps:
  - name: cloudflare
    cidrs:
      - "103.21.244.0/22"
      - "103.22.200.0/22"
      - "173.245.48.0/20"
  - name: internal-lb
    description: Internal LB egress (single address)
    cidrs:
      - "10.1.0.10"
```

**Layer 2 (secondary): Set `maxTrustedHops` as a hard ceiling**

Even if Layer 1 is misconfigured, set `maxTrustedHops = actual proxy-chain depth` to limit how far an attacker can push the result with a forged trusted-looking prefix:

```yaml
trustedIps:
  - name: cloudflare
    cidrs:
      - "103.21.244.0/22"   # Cloudflare (1 hop)
  - name: internal-lb
    cidrs:
      - "10.1.0.10"         # Internal LB (1 hop)
maxTrustedHops: 2         # Only trust 2 hops; attacker cannot push result leftward
                          # by padding more "trusted" IPs
```

### When `maxTrustedHops` Can Be Omitted

- `recursive: false`: has no effect (non-recursive takes rightmost only)
- `trustedIps` is already tight to a handful of specific IPs / small CIDRs and is audited
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
1. Check if client_addr is in trustedIps
   ├─ No → Use client_addr as real IP directly
   └─ Yes → Continue to step 2

2. Extract IP list from realIpHeader
   e.g.: X-Forwarded-For: "203.0.113.1, 198.51.100.2, 192.168.1.1"

3. Recursive lookup (if recursive=true)
   Traverse from right to left:
   ├─ 192.168.1.1 → In trustedIps ✓ Continue
   ├─ 198.51.100.2 → In trustedIps ✓ Continue
   └─ 203.0.113.1 → Not in trustedIps ✗ This is the real IP!

4. Update ctx.request_info.remote_addr (via set_remote_addr method)
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
Edgion Gateway (client_addr: 192.168.1.1)
```

**Configuration:**
```yaml
trustedIps:
  - name: load-balancer
    cidrs:
      - "192.168.0.0/16"
  - name: cdn
    cidrs:
      - "198.51.100.0/24"
realIpHeader: "X-Forwarded-For"
recursive: true
```

**Result:** `remote_addr = 203.0.113.1`

#### Scenario 2: Cloudflare CDN

```yaml
trustedIps:
  - name: cloudflare
    cidrs:
      - "173.245.48.0/20"
      - "103.21.244.0/22"
      - "103.22.200.0/22"
      # ... more Cloudflare IP ranges
realIpHeader: "CF-Connecting-IP"
recursive: false
```

## Usage Examples

### Example 1: Basic Configuration

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-basic
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
          - name: private-ranges
            cidrs:
              - "10.0.0.0/8"
              - "172.16.0.0/12"
              - "192.168.0.0/16"
```

### Example 2: Cloudflare Configuration

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-cloudflare
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
          - name: cloudflare
            cidrs:
              - "173.245.48.0/20"
              - "103.21.244.0/22"
              - "103.22.200.0/22"
              - "103.31.4.0/22"
              - "141.101.64.0/18"
              - "108.162.192.0/18"
              - "190.93.240.0/20"
              - "188.114.96.0/20"
              - "197.234.240.0/22"
              - "198.41.128.0/17"
        realIpHeader: "CF-Connecting-IP"
        recursive: false
```

### Example 3: Multi-Tier Proxy

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-multi-tier
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
          - name: internal
            description: Internal proxy and private networks
            cidrs:
              - "10.0.0.0/8"
              - "172.16.0.0/12"
              - "192.168.0.0/16"
          - name: cdn
            cidrs:
              - "198.51.100.0/24"
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
3. **Order**: This plugin should execute before other plugins that use `remote_addr` (such as rate limiting, IP restriction)
4. **Global config**: If both global `realIp` and plugin-level `RealIp` are configured, the plugin configuration overrides the global one
5. **Real-time updates**: The plugin directly modifies `ctx.request_info.remote_addr`; subsequent plugins and access logs will use the updated value

## FAQ

### Q: Why distinguish between client_addr and remote_addr?

A: 
- `client_addr`: The direct source IP of the TCP connection (usually the load balancer)
- `remote_addr`: The extracted real client IP (used for business logic)

### Q: How can I verify the configuration is working?

A: Check the `remote_addr` field in request logs, or check the `X-Real-IP` Header in the backend service.

### Q: Can I trust multiple CDNs simultaneously?

A: Yes, add each CDN as its own group under `trustedIps` (one `IpGroup` per CDN, with its ranges in `cidrs`).

### Q: Does it support dynamic updates?

A: Yes, after updating the EdgionPlugins resource, the configuration is automatically hot-reloaded.

## Related Documentation

- [IP Restriction Plugin](../http-route/filters/edgion-plugins/ip-restriction.md) - IP allow/deny lists (commonly used with RealIp)
- [HTTPRoute Overview](../http-route/overview.md) - HTTPRoute configuration overview
- [Nginx ngx_http_realip_module](https://nginx.org/en/docs/http/ngx_http_realip_module.html) - Nginx real IP module reference
- [Cloudflare IP Ranges](https://www.cloudflare.com/ips/) - Cloudflare CDN IP ranges
