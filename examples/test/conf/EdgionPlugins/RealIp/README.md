# RealIp Plugin Test

This test suite validates the RealIp plugin functionality for extracting real client IP from headers.

## Configuration

### EdgionPlugins: real-ip-test

```yaml
trustedIps:
  - "10.0.0.0/8"
  - "172.16.0.0/12"
  - "192.168.0.0/16"
  - "127.0.0.1/32"
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
1. Check if client_addr is in trusted_ips
2. If trusted, extract real IP from X-Forwarded-For header
3. Traverse from right to left to find first non-trusted IP
4. Update ctx.request_info.remote_addr (Note: current implementation logs but doesn't update yet)

## Backend Response

The test backend should echo headers including:
- X-Real-IP: The extracted real IP address
- X-Forwarded-For: Updated with client IP appended
