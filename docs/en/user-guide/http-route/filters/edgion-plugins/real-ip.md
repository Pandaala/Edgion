# Real IP Plugin

> **🔌 Edgion extension**
> 
> RealIp is a client-IP extraction plugin provided by the `EdgionPlugins` CRD; it is not part of standard Gateway API.

## What does RealIp solve?

When the gateway sits behind a reverse proxy, CDN, or cloud load balancer, the TCP connection comes from the proxy, not the end user. The proxy records the original client address in a header such as `X-Forwarded-For` or `X-Real-IP`. The RealIp plugin reads that header and resolves the **real client IP**, so that downstream plugins and logging see the user's address instead of the proxy's.

The plugin is nginx-compatible (`ngx_http_realip_module` semantics): it only trusts the header when the connecting peer is a configured trusted proxy, and can walk the `X-Forwarded-For` chain right-to-left to skip trusted hops. The resolved IP is stored on the request context and is what later plugins — for example [IP Restriction](./ip-restriction.md) — treat as the client IP.

## Quick Start

### Simplest configuration

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-plugin
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
          - name: internal
            cidrs:
              - "10.0.0.0/8"
        realIpHeader: "X-Forwarded-For"
        recursive: true
```

### Default-value notes

| Field | Default | Meaning |
|-------|---------|---------|
| `trustedIps` | **Required** | Must be non-empty — there is no default trust |
| `realIpHeader` | `X-Forwarded-For` | Header to read the client IP from |
| `recursive` | `true` | Walk the chain right-to-left, skipping trusted IPs |
| `maxTrustedHops` | none | No cap on the number of trusted hops to skip |

---

## Configuration parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `trustedIps` | Array\<Object\> | **Required** | Trusted proxy IP groups. Real-IP extraction only happens when the connecting peer is in one of these groups. Must be non-empty. See the sub-table |
| `realIpHeader` | String | `X-Forwarded-For` | Header to extract the real IP from. Common values: `X-Forwarded-For`, `X-Real-IP`, `CF-Connecting-IP`, `True-Client-IP` |
| `recursive` | Boolean | `true` | Enable nginx-style right-to-left traversal of `X-Forwarded-For`, skipping trusted IPs until the first non-trusted IP is found |
| `maxTrustedHops` | Integer | none | Cap the number of trusted hops to skip during recursive traversal (Envoy's `xff_num_trusted_hops` concept). When set to `N`, traversal stops after skipping `N` trusted hops. Only takes effect when `recursive: true`. When unset, there is no cap (nginx-compatible) |

### `trustedIps[]` group fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | String | **Required** | Group name (surfaced in debug logs) |
| `cidrs` | Array\<String\> | **Required** | IP addresses or CIDR ranges (e.g. `10.0.0.0/8`, `192.168.1.1`). Each entry must be a valid IP or CIDR |
| `description` | String | none | Optional human-readable description |

### How resolution works

- The plugin first checks whether the connecting peer is in `trustedIps`. If it is **not** trusted, the peer address is used as-is and the header is ignored (a non-trusted client cannot spoof its IP through the header).
- When the peer is trusted and `recursive: true` (default), the `X-Forwarded-For` chain is walked **right to left**, skipping any IP in `trustedIps`, and the first non-trusted IP is returned.
- If every IP in the chain is trusted, the connecting peer address is used as a fallback.
- `maxTrustedHops` bounds the traversal: it is a safety net for when `trustedIps` is broader than your real proxy chain (e.g. you trust all of `10.0.0.0/8` but attackers can also reach the gateway from that range and forge long chains).

---

## Common configuration scenarios

### 1. Single trusted load balancer (X-Real-IP, non-recursive)

When exactly one trusted LB injects `X-Real-IP`, read it directly.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-xrealip
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
          - name: lb
            cidrs:
              - "10.0.0.0/8"
        realIpHeader: "X-Real-IP"
        recursive: false
```

### 2. Multi-hop proxy chain (recursive XFF)

Trust your internal proxy ranges and resolve past them.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-recursive
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
          - name: internal
            cidrs:
              - "10.0.0.0/8"
              - "192.168.1.0/24"
        realIpHeader: "X-Forwarded-For"
        recursive: true
```

### 3. CDN with a bounded hop count (Cloudflare)

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
        realIpHeader: "CF-Connecting-IP"
        recursive: true
        # Skip at most one trusted hop
        maxTrustedHops: 1
```

---

## Interaction with IP Restriction

RealIp and [IP Restriction](./ip-restriction.md) are commonly paired. RealIp resolves the true client IP and stores it on the request context; IP Restriction then reads that resolved address (`get_real_client_ip()`) for its allow/deny decision. Order matters — both run in the request phase, so list RealIp **before** IpRestriction so the allow/deny check sees the real client IP rather than the proxy's:

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-then-restrict
  namespace: production
spec:
  requestPlugins:
    # 1. Resolve the real client IP from the trusted proxy chain
    - type: RealIp
      config:
        trustedIps:
          - name: internal
            cidrs:
              - "10.0.0.0/8"
        realIpHeader: "X-Forwarded-For"
        recursive: true
    # 2. Allow/deny based on the resolved client IP
    - type: IpRestriction
      config:
        allow:
          - name: office
            cidrs:
              - "192.168.1.0/24"
```

---

## FAQ

### Q1: My allow-list keeps blocking real users behind a proxy?

**A**: Add RealIp before IP Restriction. Without it, the gateway sees the proxy's address, not the user's, so your allow-list never matches the real client.

### Q2: Why is a spoofed `X-Forwarded-For` still trusted?

**A**: It should not be. The header is only honored when the connecting peer is in `trustedIps`, and recursive traversal skips trusted hops to reach the first non-trusted IP. If you trust too broad a range (e.g. all of `10.0.0.0/8`), narrow `trustedIps` or set `maxTrustedHops` so attackers in that range cannot forge an arbitrarily long chain.

### Q3: The gateway rejected my config?

**A**: `trustedIps` must be non-empty, `realIpHeader` must not be blank, and every entry in `cidrs` must be a valid IP or CIDR (for example, `10.0.0.0/8`, not `10.0.0/8`). The plugin reports the offending field at parse time.

---

## Complete configuration example

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - "api.example.com"
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
            name: real-ip-plugin
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-plugin
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
          - name: internal
            cidrs:
              - "10.0.0.0/8"
              - "192.168.1.0/24"
        realIpHeader: "X-Forwarded-For"
        recursive: true
```

## Related links

- [IP Restriction Plugin](./ip-restriction.md)
- [Filters overview](../README.md)
