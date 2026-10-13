# RealIp Plugin Test

This test suite validates the RealIp plugin functionality for extracting real client IP from headers.

## Configuration

### EdgionPlugins: real-ip-test

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
      - name: private-ranges
        cidrs:
        - 10.0.0.0/8
        - 172.16.0.0/12
        - 192.168.0.0/16
        - 127.0.0.1/32
---
trustedIpRefs:
  - name: realip-proxies-1
    namespace: default
realIpHeader: "X-Forwarded-For"
recursive: true
```

## Test Scenarios

### 1. X-Forwarded-For with Trusted Proxies
- Request: X-Forwarded-For: "203.0.113.1, 198.51.100.2, 192.168.1.1"
- Client: 127.0.0.1 (trusted)
- Expected: Extract "203.0.113.1" (first non-trusted IP from right)

### 2. Direct Connection (No Proxy)
- Request: No X-Forwarded-For header
- Client: 203.0.113.1 (not trusted)
- Expected: Use client address "203.0.113.1"

### 3. All Trusted IPs
- Request: X-Forwarded-For: "192.168.1.1, 10.0.0.1"
- Client: 127.0.0.1 (trusted)
- Expected: Use leftmost IP "192.168.1.1"

## Verification

The plugin will:
1. Check if the current downstream peer is in trusted IpList entries
2. If trusted, extract real IP from X-Forwarded-For header
3. Traverse from right to left to find first non-trusted IP
4. Update the request's resolved RemoteIp (the `remote_ip` access-log field)

## Backend Response

The test backend should echo headers including:
- X-Real-IP: The default upstream header carrying the extracted RemoteIp value
- `spec.forwardedHeaders.remoteIpHeader`: Optional custom upstream header name for RemoteIp
- X-Forwarded-For: Updated with client IP appended

The plugin references `edgion-default/realip-proxies`; its original four ranges now live in the single-resource IpList fixture. Every trusted reference must be available. Missing lists fall back to the effective downstream peer; malformed IpList entries use the central partial-good compiler. No inline trust collections remain in the plugin.
