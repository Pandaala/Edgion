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
        cidrs:
        - 10.0.0.0/8
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
        trustedIpRefs:
          - name: realip-proxies-1
            namespace: production
        realIpHeader: "X-Forwarded-For"
        recursive: true
```

### Default-value notes

| Field | Default | Meaning |
|-------|---------|---------|
| `trustedIpRefs` | **Required** | Must be non-empty — there is no default trust |
| `realIpHeader` | `X-Forwarded-For` | Header to read the client IP from |
| `recursive` | `true` | Walk the chain right-to-left, skipping trusted IPs |
| `maxTrustedHops` | none | No cap on the number of trusted hops to skip |

---

## Configuration parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `trustedIpRefs` | Array\<Object\> | **Required** | Nonempty references to trusted proxy IpLists; all referenced lists must be available. See the reference fields below |
| `realIpHeader` | String | `X-Forwarded-For` | Header to extract the real IP from. Must be a valid HTTP header field name (RFC 9110 `token` charset) of at most 256 bytes; an invalid name is rejected at admission. Common values: `X-Forwarded-For`, `X-Real-IP`, `CF-Connecting-IP`, `True-Client-IP` |
| `recursive` | Boolean | `true` | Enable nginx-style right-to-left traversal of `X-Forwarded-For`, skipping trusted IPs until the first non-trusted IP is found |
| `maxTrustedHops` | Integer | none | Cap the number of trusted hops to skip during recursive traversal (Envoy's `xff_num_trusted_hops` concept). When set to `N`, traversal stops after skipping `N` trusted hops. Only takes effect when `recursive: true`. When unset, there is no cap (nginx-compatible) |

### `trustedIpRefs[]`

A Kubernetes update rejected by API admission for required references, minimum size, target namespace or header constraints never reaches the Controller/Gateway; the previous admitted generation remains active. Invalid-generation replacement below refers to input that reaches typed publication (for example FS input or preserved mixed legacy keys), not an API-rejected update.

References are ordered `EdgionConfigData` IpLists. Put IP/CIDR collections in each IpList's `items` (IpGroups), then reference them by `name` and optional `namespace`. HTTP plugin references default to the actual owning plugin namespace. Global `spec.realIp` references require an explicit namespace and current `Cluster` visibility when ReferenceGrant validation is enabled; namespaced grants do not authorize this cluster owner. Disabling that validation preserves identity, IpList type and enabled checks.

Every referenced list must be available before trusting the effective downstream peer or any forwarded hop. A missing, disabled, denied or wrong-type list disables trust for this decision and uses the PROXY-effective peer, even when another list matches. Content updates, deletion, visibility changes and restoration affect subsequent decisions without copying CIDRs into the consumer. One captured view covers the peer and all hops. Invalid entries in an otherwise valid IpList are excluded by the central partial-good compiler; an empty/all-invalid matcher trusts no addresses.

| Field | Meaning |
| --- | --- |
| `name` | Required IpList resource name |
| `namespace` | HTTP: defaults to the owning namespace; global EGC: required |

Removed inline `trustedIps` and unknown fields are invalid even when null or empty. Invalid policies that reach typed publication replace the previous generation with a sealed plugin failure or disable global extraction. A clean generation restores the policy. Global EGC `Accepted` reports intrinsic structure only, never deployment-local list availability.

### How resolution works

- The plugin first checks whether the connecting peer is in `trustedIpRefs`. If it is **not** trusted, the peer address is used as-is and the header is ignored (a non-trusted client cannot spoof its IP through the header).
- When the peer is trusted and `recursive: true` (default), the `X-Forwarded-For` chain is walked **right to left**, skipping any IP in `trustedIpRefs`, and the first non-trusted IP is returned.
- If every parsed hop in a recursive chain is trusted, the leftmost parsed hop is returned. Nonrecursive mode uses the peer when the rightmost parsed hop is trusted.
- `maxTrustedHops` bounds the traversal: it is a safety net for when `trustedIpRefs` is broader than your real proxy chain (e.g. you trust all of `10.0.0.0/8` but attackers can also reach the gateway from that range and forge long chains).

---

## Common configuration scenarios

### 1. Single trusted load balancer (X-Real-IP, non-recursive)

When exactly one trusted LB injects `X-Real-IP`, read it directly.

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
      - name: lb
        cidrs:
        - 10.0.0.0/8
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-xrealip
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIpRefs:
          - name: realip-proxies-1
            namespace: production
        realIpHeader: "X-Real-IP"
        recursive: false
```

### 2. Multi-hop proxy chain (recursive XFF)

Trust your internal proxy ranges and resolve past them.

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
        cidrs:
        - 10.0.0.0/8
        - 192.168.1.0/24
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-recursive
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

### 3. CDN with a bounded hop count (Cloudflare)

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
        recursive: true
        # Skip at most one trusted hop
        maxTrustedHops: 1
```

---

## Interaction with IP Restriction

RealIp and [IP Restriction](./ip-restriction.md) are commonly paired. RealIp resolves `RemoteIp` and stores it on the request context; IP Restriction then reads that resolved address (`get_remote_ip()`) for its allow/deny decision. Order matters — both run in the request phase, so list RealIp **before** IpRestriction so the allow/deny check sees the resolved client rather than the proxy's:

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
        cidrs:
        - 10.0.0.0/8
---
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
        trustedIpRefs:
          - name: realip-proxies-1
            namespace: production
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

**A**: It should not be. The header is only honored when the connecting peer is in `trustedIpRefs`, and recursive traversal skips trusted hops to reach the first non-trusted IP. If you trust too broad a range (e.g. all of `10.0.0.0/8`), narrow `trustedIpRefs` or set `maxTrustedHops` so attackers in that range cannot forge an arbitrarily long chain.

### Q3: The gateway rejected my config?

**A**: `trustedIpRefs` must be nonempty and `realIpHeader` a valid HTTP field name. IpList entries are compiled centrally: valid entries remain usable, invalid entries are excluded, and all referenced lists must be available before trusting forwarded input.

---

## Migration to DirectPeerIp and RemoteIp

The pre-release rename is intentionally hard: `client_addr`/`client-addr` maps
to `DirectPeerIp`/`direct_peer_ip`, while `remote_addr`/`remote-addr` maps to
`RemoteIp`/`remote_ip`. Update `IpSource`, `KeyGet`, templates, DSL, access-log
queries, and backend assertions together; no compatibility aliases or dual log
fields are retained. The DSL rename `req.client_ip` → `req.direct_peer_ip` is an
exact identity rename for callers that intentionally need the physical peer;
ordinary client identity, rate-limit, and experiment bucketing use cases should
choose `req.remote_ip`. `RealIp` remains the name of the resolver mechanism. The
upstream propagation header is configured independently through
`EdgionGatewayConfig.spec.forwardedHeaders.remoteIpHeader` and defaults to
`X-Real-IP`.

## Complete configuration example

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
        cidrs:
        - 10.0.0.0/8
        - 192.168.1.0/24
---
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
        trustedIpRefs:
          - name: realip-proxies-1
            namespace: production
        realIpHeader: "X-Forwarded-For"
        recursive: true
```

## Related links

- [IP Restriction Plugin](./ip-restriction.md)
- [Filters overview](../README.md)
