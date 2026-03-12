# RealIp Plugin

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
  namespace: default
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
          - "10.0.0.0/8"
          - "172.16.0.0/12"
          - "192.168.0.0/16"
        realIpHeader: "X-Forwarded-For"
        recursive: true
```

### Configuration Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `trustedIps` | string[] | Yes | - | List of trusted proxy IP addresses or CIDR ranges |
| `realIpHeader` | string | No | `"X-Forwarded-For"` | Header name to extract the real IP from |
| `recursive` | boolean | No | `true` | Whether to enable recursive lookup (Nginx-style) |

#### trustedIps

List of trusted proxy IP addresses or CIDR ranges. Corresponds to Nginx's `set_real_ip_from` directive.

- Supports single IP addresses: `"192.168.1.1"`
- Supports CIDR ranges: `"10.0.0.0/8"`
- Supports IPv6: `"2001:db8::/32"`

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
  - "192.168.0.0/16"  # Load balancer
  - "198.51.100.0/24" # CDN
realIpHeader: "X-Forwarded-For"
recursive: true
```

**Result:** `remote_addr = 203.0.113.1`

#### Scenario 2: Cloudflare CDN

```yaml
trustedIps:
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
  namespace: default
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
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
  namespace: default
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
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
  namespace: default
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
          - "10.0.0.0/8"       # Internal proxy
          - "172.16.0.0/12"    # Private network
          - "192.168.0.0/16"   # Local network
          - "198.51.100.0/24"  # CDN
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

A: Yes, add all CDN IP ranges to the `trustedIps` list.

### Q: Does it support dynamic updates?

A: Yes, after updating the EdgionPlugins resource, the configuration is automatically hot-reloaded.

## Related Resources

- [Nginx ngx_http_realip_module](https://nginx.org/en/docs/http/ngx_http_realip_module.html)
- [APISIX real-ip plugin](https://apisix.apache.org/docs/apisix/plugins/real-ip/)
- [Cloudflare IP Ranges](https://www.cloudflare.com/ips/)
