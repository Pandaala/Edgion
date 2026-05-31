# IP Restriction Plugin

> **🔌 Edgion extension**
> 
> IpRestriction is an access control plugin provided by the `EdgionPlugins` CRD and is not part of the standard Gateway API.

## What is IP Restriction?

The IP Restriction plugin controls which IP addresses or CIDR ranges can access your API, providing allowlist and denylist functionality.

`allow` and `deny` are lists of **named IP groups**. Each group has a `name` (surfaced in deny logs for audit/attribution) and a `cidrs` list of IP/CIDR strings. At least one of `allow` or `deny` must be non-empty.

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
        - name: internal
          cidrs:
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
        - name: blocklist
          cidrs:
            - "203.0.113.50"     # Deny single malicious IP
            - "198.51.100.0/24"  # Deny entire malicious subnet
```

**Effect**: Only the IPs in the list are denied; all other IPs can access (because `defaultAction` is `allow`).

---

## Configuration Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `allow` | Array\<Object\> | none | Allowlist groups (whitelist). When non-empty, only IPs in an allow group are permitted. Each entry is an IP group (see sub-table). |
| `deny` | Array\<Object\> | none | Denylist groups (blacklist). Highest priority: IPs in a deny group are always denied. |
| `ipSource` | Enum | `clientIp` | IP source. `clientIp`: reads `remote_addr` (pre-processed by the RealIp plugin or global `spec.realIp`; falls back to the TCP peer IP if neither is configured). `remoteAddr`: always the TCP connection peer address (never forgeable). |
| `status` | Integer | `403` | HTTP status code returned when denied (100..=599). |
| `message` | String | none | Custom message returned when denied. |
| `defaultAction` | Enum | `allow` | Action when an IP matches no rule. Allowed values: `allow` / `deny`. |

At least one of `allow` or `deny` must be non-empty, otherwise the configuration is rejected.

### IP group sub-fields (`allow[]` / `deny[]`)

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | String | — | Group name. Required. Surfaces in deny logs for audit/attribution. |
| `description` | String | none | Human-readable description (not used by the runtime). |
| `cidrs` | Array\<String\> | `[]` | IP addresses or CIDR ranges (IPv4 and IPv6). |

---

## Priority Rules

The plugin uses a three-tier priority system consistent with Nginx:

```
1. Deny takes priority (highest priority)
   └─> If IP is in a deny group, deny immediately

2. Allow comes next
   └─> If IP is in an allow group, allow access
   └─> If allow is configured but IP is not in any allow group, deny

3. Default Action (fallback)
   └─> If nothing matches, use defaultAction
```

### Example

```yaml
allow:
  - name: internal
    cidrs: ["10.0.0.0/8"]
deny:
  - name: blocked
    cidrs: ["10.0.0.100"]
defaultAction: "deny"
```

| Client IP | Matched Rule | Result |
|-----------|-------------|--------|
| `10.0.0.100` | In deny group | ❌ Denied (deny takes priority) |
| `10.0.0.50` | In allow group | ✅ Allowed |
| `10.1.2.3` | In allow group | ✅ Allowed |
| `192.168.1.1` | Not in any group | ❌ Denied (defaultAction: deny) |

---

## Common Configuration Scenarios

### 1. Internal Network Only API

Allow only company internal network access:

```yaml
filters:
  - type: IpRestriction
    config:
      allow:
        - name: rfc1918
          cidrs:
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
        - name: office
          cidrs:
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
        - name: abuse
          cidrs:
            - "203.0.113.50"      # Malicious attacker
            - "198.51.100.100"    # Malicious crawler
            - "192.0.2.0/24"      # Malicious subnet
      message: "Your IP has been blocked"
      status: 403
      defaultAction: "allow"  # All other IPs allowed
```

### 4. Combined Mode: Allow Subnet but Exclude Specific IPs

Allow an entire subnet but exclude a few IPs (deny wins over allow):

```yaml
filters:
  - type: IpRestriction
    config:
      allow:
        - name: subnet
          cidrs:
            - "10.0.0.0/16"       # Allow entire subnet
      deny:
        - name: excluded
          cidrs:
            - "10.0.0.100"        # Exclude test machine 1
            - "10.0.0.200"        # Exclude test machine 2
```

**Results**:
- `10.0.0.50` → ✅ Allowed (in allow, not in deny)
- `10.0.0.100` → ❌ Denied (in deny, deny takes priority)
- `192.168.1.1` → ❌ Denied (not in allow)

### 5. Multiple Named Groups (Partners)

Use separate named groups so deny logs attribute the matched group:

```yaml
filters:
  - type: IpRestriction
    config:
      allow:
        - name: partner-a
          cidrs: ["203.0.113.0/24"]
        - name: partner-b
          cidrs: ["198.51.100.50"]
        - name: partner-c
          cidrs: ["192.0.2.100"]
      message: "Access restricted to authorized partners"
      status: 403
```

### 6. Behind a Load Balancer (trust the real client IP)

When the gateway sits behind a proxy/load balancer, the TCP peer is the proxy. Configure the RealIp plugin first to derive the true client IP from `X-Forwarded-For` / `X-Real-IP`; then `ipSource: clientIp` matches against the real IP.

```yaml
filters:
  # RealIp normalizes remote_addr from trusted proxies first
  - type: RealIp
    config:
      trustedIps:
        - name: lb
          cidrs:
            - "10.0.0.0/8"
  # IpRestriction reads the normalized client IP
  - type: IpRestriction
    config:
      ipSource: "clientIp"
      allow:
        - name: clients
          cidrs: ["203.0.113.0/24"]
```

> **Warning:** `ipSource: clientIp` is only as trustworthy as the upstream that sets `X-Forwarded-For`. Without RealIp configured, `clientIp` falls back to the TCP peer IP (safe, not forgeable). If clients can reach the gateway directly, do not trust forwarded headers — use `ipSource: remoteAddr` or configure RealIp with the exact trusted-proxy ranges.

---

## IP Source Selection

### `ipSource: clientIp` (Default)

Reads `remote_addr`, which the RealIp plugin or global `spec.realIp` pre-processes from proxy headers. Without RealIp configured, it falls back to the TCP connection IP (not forgeable).

```yaml
filters:
  - type: IpRestriction
    config:
      ipSource: "clientIp"  # Default
      allow:
        - name: clients
          cidrs: ["203.0.113.0/24"]
```

### `ipSource: remoteAddr`

Always uses the TCP connection's peer address (direct connection IP); never forgeable.

**Use cases**:
- Direct deployment without proxy/load balancer
- Restricting the proxy/load balancer server IPs themselves

```yaml
filters:
  - type: IpRestriction
    config:
      ipSource: "remoteAddr"
      allow:
        - name: lb
          cidrs: ["10.0.0.0/8"]  # Only allow internal load balancers
```

---

## CIDR Notation

### Single IP

```yaml
allow:
  - name: hosts
    cidrs:
      - "192.168.1.100"      # Single IP
      - "203.0.113.50"
```

### CIDR Subnets

```yaml
allow:
  - name: subnets
    cidrs:
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
  - name: ipv6
    cidrs:
      - "2001:db8::1"                    # Single IPv6
      - "2001:db8::/32"                  # IPv6 subnet
      - "fe80::/10"                      # Link-local address
```

---

## Custom Deny Response

### Custom Status Code and Message

```yaml
filters:
  - type: IpRestriction
    config:
      allow:
        - name: internal
          cidrs: ["10.0.0.0/8"]
      status: 404  # Pretend the resource doesn't exist (hide the API)
      message: "Not Found"
```

Or return a friendlier message:

```yaml
filters:
  - type: IpRestriction
    config:
      allow:
        - name: office
          cidrs: ["192.168.1.0/24"]
      status: 403
      message: "Access denied. Please contact your administrator."
```

---

## Security Best Practices

### ✅ Recommended

1. **Principle of least privilege** — only allow the IPs you need; prefer an explicit allow-list over relying on deny alone.

   ```yaml
   allow:
     - name: hosts
       cidrs:
         - "192.168.1.100"
         - "192.168.1.101"
   ```

2. **Use the smallest CIDR range possible** — `10.0.1.0/24` rather than `10.0.0.0/8`.

3. **Review rules regularly** — remove IPs that are no longer needed; update changed office IPs.

4. **Pair `clientIp` with RealIp** — only trust forwarded client IPs when the RealIp plugin (or global `spec.realIp`) sanitizes them from known trusted proxies. Otherwise use `remoteAddr`.

5. **Combine with authentication** — IP restriction is defense-in-depth, not a substitute for authentication.

   ```yaml
   filters:
     - type: IpRestriction
       config:
         allow:
           - name: internal
             cidrs: ["10.0.0.0/8"]
     - type: BasicAuth
       config:
         secretGroups:
           - name: default
             secretRefs:
               - name: admin-users
   ```

### ❌ Avoid

1. **Overly broad ranges in production** (for example `0.0.0.0/0`, which allows all IPs).
2. **Relying solely on IP restriction for authentication** — IPs can be spoofed, especially on internal networks.
3. **Trusting `clientIp` without RealIp** on a gateway that clients can reach directly (forwarded-header spoofing bypass).
4. **Forgetting IPv6** — configure both families when IPv6 traffic is possible.

---

## Troubleshooting

### Issue 1: Legitimate IPs Are Denied

**Cause**:
- CIDR range misconfigured
- Wrong `ipSource` (behind a proxy but using `remoteAddr` without RealIp)

**Solution**: Check the request logs for the actual matched IP and the `X-Forwarded-For` header, then align `ipSource` (and configure RealIp if behind a proxy).

### Issue 2: Cannot Obtain Real IP

**Cause**: The proxy/load balancer is not setting `X-Forwarded-For`, or RealIp is not configured.

**Solution**: Configure the RealIp plugin with the trusted proxy ranges, or temporarily use `ipSource: remoteAddr` and allow the load balancer IP.

### Issue 3: Rules Not Taking Effect

**Cause**:
- Plugin not bound to the route
- YAML formatting error

**Solution**:
```bash
kubectl get edgionplugins -A
kubectl describe edgionplugins <name>
```

---

## Complete Example

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: protected-admin
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - "api.example.com"
  rules:
    # Admin interface: strict IP allowlist
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
  namespace: production
spec:
  requestPlugins:
    # IP allowlist
    - type: IpRestriction
      config:
        ipSource: "clientIp"
        allow:
          - name: internal
            cidrs:
              - "10.0.0.0/8"
          - name: office
            cidrs:
              - "203.0.113.10"
              - "203.0.113.20"
        deny:
          - name: excluded
            cidrs:
              - "10.0.0.100"
        message: "Admin access restricted to authorized networks"
        status: 403
    # Additional authentication
    - type: BasicAuth
      config:
        secretGroups:
          - name: default
            secretRefs:
              - name: admin-users
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: public-security
  namespace: production
spec:
  requestPlugins:
    # IP denylist
    - type: IpRestriction
      config:
        ipSource: "clientIp"
        deny:
          - name: abuse
            cidrs:
              - "203.0.113.50"
              - "198.51.100.0/24"
        defaultAction: "allow"
        message: "Your IP has been blocked due to suspicious activity"
        status: 403
```

---

## Related Links

- [RealIp](./real-ip.md) — Real client IP extraction (configure first when behind a proxy)
- [RateLimit](./rate-limit.md) — Rate limiting
- [RequestRestriction](./request-restriction.md) — Request restriction
- [Basic Auth](./basic-auth.md) — Combine with authentication
