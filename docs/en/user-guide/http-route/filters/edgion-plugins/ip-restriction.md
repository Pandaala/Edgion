# IP Restriction Plugin

> **🔌 Edgion Extension**
> 
> IpRestriction is an access control plugin provided by the `EdgionPlugins` CRD and is not part of the standard Gateway API.

## What is IP Restriction?

The IP Restriction plugin controls which IP addresses or CIDR ranges can access your API, providing allowlist and denylist functionality.

**Use cases**:
- Only allow internal network IPs to access admin interfaces
- Block specific malicious IP addresses
- Allow specific partner IPs to access the API
- Restrict payment interfaces to application server access only

## Quick Start

### Allowlist Mode (Allow Specific IPs Only)

```yaml
filters:
  - type: IpRestriction
    config:
      allow:
        - "192.168.1.0/24"  # Allow entire subnet
        - "10.0.0.100"       # Allow single IP
```

**Effect**: Only `192.168.1.x` and `10.0.0.100` can access; all other IPs are denied.

### Denylist Mode (Deny Specific IPs)

```yaml
filters:
  - type: IpRestriction
    config:
      deny:
        - "203.0.113.50"     # Deny single malicious IP
        - "198.51.100.0/24"  # Deny entire malicious subnet
```

**Effect**: Only the IPs in the list are denied; all other IPs can access.

---

## Configuration Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `allow` | Array | None | Allowlist. List of IP addresses or CIDRs. Example: `["192.168.1.0/24", "10.0.0.1"]` |
| `deny` | Array | None | Denylist. List of IP addresses or CIDRs. Example: `["203.0.113.50"]` |
| `ipSource` | String | `"clientIp"` | IP source. `clientIp`: extract real IP from proxy headers; `remoteAddr`: use TCP connection address |
| `status` | Integer | `403` | HTTP status code returned when denied |
| `message` | String | None | Custom message returned when denied |
| `defaultAction` | String | `"allow"` | Default action when IP doesn't match any rule. `allow` or `deny` |

---

## Priority Rules

The plugin uses a three-tier priority system consistent with Nginx:

```
1. Deny takes priority (highest priority)
   └─> If IP is in the deny list, deny immediately

2. Allow comes next
   └─> If IP is in the allow list, allow access
   └─> If allow is configured but IP is not in the list, deny

3. Default Action (fallback)
   └─> If nothing matches, use defaultAction
```

### Example

```yaml
allow: ["10.0.0.0/8"]
deny: ["10.0.0.100"]
defaultAction: "deny"
```

| Client IP | Matched Rule | Result |
|-----------|-------------|--------|
| `10.0.0.100` | In deny list | ❌ Denied (deny takes priority) |
| `10.0.0.50` | In allow list | ✅ Allowed |
| `10.1.2.3` | In allow list | ✅ Allowed |
| `192.168.1.1` | Not in any list | ❌ Denied (defaultAction: deny) |

---

## Common Configuration Scenarios

### 1. Internal Network Only API

Allow only company internal network access:

```yaml
filters:
  - type: IpRestriction
    config:
      allow:
        - "10.0.0.0/8"        # Private Class A
        - "172.16.0.0/12"     # Private Class B
        - "192.168.0.0/16"    # Private Class C
      message: "Access denied: Internal network only"
      status: 403
```

### 2. Office Allowlist

Allow only fixed office IPs:

```yaml
filters:
  - type: IpRestriction
    config:
      allow:
        - "203.0.113.10"      # Office IP 1
        - "203.0.113.20"      # Office IP 2
        - "198.51.100.0/24"   # VPN subnet
      message: "Access restricted to office network"
```

### 3. Denylist: Block Malicious IPs

```yaml
filters:
  - type: IpRestriction
    config:
      deny:
        - "203.0.113.50"      # Malicious attacker
        - "198.51.100.100"    # Malicious crawler
        - "192.0.2.0/24"      # Malicious subnet
      message: "Your IP has been blocked"
      status: 403
      defaultAction: "allow"  # All other IPs allowed
```

### 4. Combined Mode: Allow Subnet but Exclude Specific IPs

Allow an entire subnet but exclude a few IPs (e.g., test machines):

```yaml
filters:
  - type: IpRestriction
    config:
      allow:
        - "10.0.0.0/16"       # Allow entire subnet
      deny:
        - "10.0.0.100"        # Exclude test machine 1
        - "10.0.0.200"        # Exclude test machine 2
```

**Results**:
- `10.0.0.50` → ✅ Allowed (in allow, not in deny)
- `10.0.0.100` → ❌ Denied (in deny, deny takes priority)
- `192.168.1.1` → ❌ Denied (not in allow)

### 5. Partner API

Allow specific partner access:

```yaml
filters:
  - type: IpRestriction
    config:
      allow:
        - "203.0.113.0/24"    # Partner A
        - "198.51.100.50"     # Partner B
        - "192.0.2.100"       # Partner C
      message: "Access restricted to authorized partners"
      status: 403
```

### 6. Multi-Tier Access Control

Different IP restrictions for different paths:

```yaml
# Admin interface: internal network only
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: admin-api
spec:
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /admin
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: admin-ip-policy
---
# Public interface: denylist mode
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: public-api
spec:
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: public-ip-policy
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: admin-ip-policy
spec:
  plugins:
    - enable: true
      plugin:
        type: IpRestriction
        config:
          allow:
            - "10.0.0.0/8"
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: public-ip-policy
spec:
  plugins:
    - enable: true
      plugin:
        type: IpRestriction
        config:
          deny:
            - "203.0.113.50"
          defaultAction: "allow"
```

---

## IP Source Selection

### `ipSource: clientIp` (Default, Recommended)

Extracts the real client IP from proxy headers (`X-Forwarded-For` or `X-Real-IP`).

**Use cases**:
- Application deployed behind CDN/load balancers
- Need to obtain the real client IP

**Example**:
```
Client IP: 203.0.113.50
 ↓
CDN/Load Balancer: 198.51.100.10
 ↓
Edgion extracts: X-Forwarded-For: 203.0.113.50
 ↓
Matches rule: 203.0.113.50
```

```yaml
filters:
  - type: IpRestriction
    config:
      ipSource: "clientIp"  # Default
      allow:
        - "203.0.113.0/24"
```

### `ipSource: remoteAddr`

Uses the TCP connection's peer address (direct connection IP).

**Use cases**:
- Direct deployment without proxy/load balancer
- Need to restrict proxy server IPs

**Example**:
```
Load Balancer: 10.0.0.50
 ↓
Edgion gets: TCP peer address: 10.0.0.50
 ↓
Matches rule: 10.0.0.50
```

```yaml
filters:
  - type: IpRestriction
    config:
      ipSource: "remoteAddr"
      allow:
        - "10.0.0.0/8"  # Only allow internal load balancers
```

---

## CIDR Notation

### Single IP

```yaml
allow:
  - "192.168.1.100"      # Single IP
  - "203.0.113.50"
```

### CIDR Subnets

```yaml
allow:
  - "192.168.1.0/24"     # 192.168.1.0 - 192.168.1.255 (256 IPs)
  - "10.0.0.0/8"         # 10.0.0.0 - 10.255.255.255 (16M IPs)
  - "172.16.0.0/12"      # 172.16.0.0 - 172.31.255.255
  - "203.0.113.0/28"     # 203.0.113.0 - 203.0.113.15 (16 IPs)
```

### CIDR Quick Reference

| CIDR | Subnet Mask | IP Count | IP Range Example |
|------|-------------|----------|-----------------|
| `/32` | 255.255.255.255 | 1 | Single IP |
| `/24` | 255.255.255.0 | 256 | x.x.x.0 - x.x.x.255 |
| `/16` | 255.255.0.0 | 65,536 | x.x.0.0 - x.x.255.255 |
| `/8` | 255.0.0.0 | 16,777,216 | x.0.0.0 - x.255.255.255 |

### IPv6 Support

```yaml
allow:
  - "2001:db8::1"                    # Single IPv6
  - "2001:db8::/32"                  # IPv6 subnet
  - "fe80::/10"                      # Link-local address
```

---

## Custom Deny Response

### Default Response

```
HTTP/1.1 403 Forbidden
Content-Type: application/json

{
  "error": "IP address not allowed"
}
```

### Custom Status Code and Message

```yaml
filters:
  - type: IpRestriction
    config:
      allow:
        - "10.0.0.0/8"
      status: 404  # Pretend the resource doesn't exist (hide the API)
      message: "Not Found"
```

Or return a friendlier message:

```yaml
filters:
  - type: IpRestriction
    config:
      allow:
        - "192.168.1.0/24"
      status: 403
      message: "Access denied. Please contact administrator at admin@example.com"
```

---

## Security Best Practices

### ✅ Recommended

1. **Principle of least privilege**
   ```yaml
   # ✅ Good: only allow needed IPs
   allow:
     - "192.168.1.100"
     - "192.168.1.101"
   
   # ❌ Bad: allow the entire internet
   # defaultAction: "allow"
   ```

2. **Use the smallest CIDR range possible**
   ```yaml
   # ✅ Good: precise range
   allow:
     - "10.0.1.0/24"
   
   # ❌ Bad: overly broad range
   allow:
     - "10.0.0.0/8"
   ```

3. **Review rules regularly**
    - Remove IPs that are no longer needed
    - Update changed office IPs

4. **Document deny reasons**
   ```yaml
   message: "Internal API - Access restricted to office network (203.0.113.0/24)"
   ```

5. **Combine with other authentication**
   ```yaml
   # IP restriction + Basic Auth = dual protection
   filters:
     - type: IpRestriction
       config:
         allow: ["10.0.0.0/8"]
     - type: BasicAuth
       config:
         secretRefs:
           - name: admin-users
   ```

### ❌ Avoid

1. **Do not use overly broad ranges in production**
   ```yaml
   # ❌ Dangerous: allow all private networks
   allow:
     - "0.0.0.0/0"  # Allows all IPs
   ```

2. **Do not rely solely on IP restriction for authentication**
    - IPs can be spoofed (especially on internal networks)
    - Should be combined with user authentication

3. **Do not forget IPv6**
   ```yaml
   # If IPv6 is supported, configure both
   allow:
     - "192.168.1.0/24"   # IPv4
     - "2001:db8::/32"    # IPv6
   ```

---

## Troubleshooting

### Issue 1: Legitimate IPs Are Denied

**Cause**:
- CIDR range misconfigured
- IP source misconfigured (`ipSource`)
- CDN/load balancer present but using `remoteAddr`

**Solution**:
```bash
# Check request logs to confirm actual IP
# Check X-Forwarded-For header

# Adjust configuration
ipSource: "clientIp"  # If behind a proxy
# Or
ipSource: "remoteAddr"  # If direct connection
```

### Issue 2: Cannot Obtain Real IP

**Cause**: CDN/load balancer not correctly setting proxy headers

**Solution**:
```yaml
# Temporary: use remoteAddr and allow load balancer IP
ipSource: "remoteAddr"
allow:
  - "10.0.0.50"  # Load balancer IP

# Long-term: configure load balancer to correctly pass X-Forwarded-For
```

### Issue 3: Rules Not Taking Effect

**Cause**:
- Plugin not correctly bound to the route
- YAML formatting error

**Solution**:
```bash
# Check resource status
kubectl get edgionplugins -A
kubectl describe edgionplugins <name>

# Check logs
kubectl logs -n edgion-system <edgion-controller-pod>
```

---

## Testing Configuration

### Using curl

```bash
# 1. Test allowlist
curl -v https://api.example.com/admin
# Should return 403 (if your IP is not in the allowlist)

# 2. Spoof IP test (requires server cooperation)
curl -H "X-Forwarded-For: 192.168.1.100" https://api.example.com/admin
# If 192.168.1.100 is in the allowlist, it should succeed
```

### Testing from Different IPs

```bash
# From the office
curl https://api.example.com/admin

# From home (should be denied)
curl https://api.example.com/admin

# From a server in the allowlist
ssh server-in-whitelist
curl https://api.example.com/admin
```

---

## Complete Example

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: protected-admin
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
  hostnames:
    - "api.example.com"
  rules:
    # Admin interface: strict IP restriction
    - matches:
        - path:
            type: PathPrefix
            value: /admin
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: admin-security
      backendRefs:
        - name: admin-service
          port: 8080
    
    # Public API: denylist mode
    - matches:
        - path:
            type: PathPrefix
            value: /api
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: public-security
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: admin-security
  namespace: default
spec:
  plugins:
    # IP allowlist
    - enable: true
      plugin:
        type: IpRestriction
        config:
          ipSource: "clientIp"
          allow:
            - "10.0.0.0/8"           # Internal network
            - "203.0.113.10"         # Office IP 1
            - "203.0.113.20"         # Office IP 2
          deny:
            - "10.0.0.100"           # Exclude test machine
          message: "Admin access restricted to authorized networks"
          status: 403
    
    # Additional authentication
    - enable: true
      plugin:
        type: BasicAuth
        config:
          secretRefs:
            - name: admin-users
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: public-security
  namespace: default
spec:
  plugins:
    # IP denylist
    - enable: true
      plugin:
        type: IpRestriction
        config:
          ipSource: "clientIp"
          deny:
            - "203.0.113.50"         # Malicious IP
            - "198.51.100.0/24"      # Malicious subnet
          defaultAction: "allow"
          message: "Your IP has been blocked due to suspicious activity"
          status: 403
```

**Test scenarios**:

```bash
# 1. Access admin interface from office - success
curl -u admin:password https://api.example.com/admin/users

# 2. Access admin interface from home - fail (IP not in allowlist)
curl https://api.example.com/admin/users
# -> 403 Forbidden

# 3. Malicious IP accesses public API - fail
curl -H "X-Forwarded-For: 203.0.113.50" https://api.example.com/api/data
# -> 403 Forbidden

# 4. Normal user accesses public API - success
curl https://api.example.com/api/data
# -> 200 OK
```
