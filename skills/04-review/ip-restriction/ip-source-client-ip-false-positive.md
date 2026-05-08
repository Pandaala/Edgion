---
name: ip-source-client-ip-false-positive
description: Decision rule for IP restriction / rate limiting ClientIp findings. Read this file first when reviewing "ClientIp can be spoofed via XFF when RealIp is not configured" findings.
---

# IpSource::ClientIp / KeyGet::ClientIp — NOT-AN-ISSUE

## Conclusion

Findings of the form "without the RealIp plugin enabled, an attacker can bypass the IP allowlist/rate limit by spoofing `X-Forwarded-For`" rest on a wrong premise; classified as NOT-AN-ISSUE.

## Code Path (key evidence)

```
IpSource::ClientIp  → session.remote_addr() → ctx.request_info.remote_addr
KeyGet::ClientIp    → self.remote_addr()    → ctx.request_info.remote_addr
```

`remote_addr` is initialized in `src/core/gateway/routes/http/proxy_http/pg_request_filter.rs:261-266`:

```rust
ctx.request_info.remote_addr = if let Some(extractor) = &edgion_http.real_ip_extractor {
    extractor.extract_real_ip(&client_addr_str, req_header)
} else {
    // No extractor configured, use client_addr (already IP only)
    ctx.request_info.client_addr.clone()
};
```

**When `real_ip_extractor` is not configured, `remote_addr` is set directly to `client_addr` (the TCP connection IP); XFF/X-Real-IP request headers are not read at all.**

## Why the doc comment looked like "reads XFF"

The original doc comment for `IpSource::ClientIp` read "Extract from X-Forwarded-For (first IP) or fallback to X-Real-IP", which is misleading.
In reality, reading XFF/X-Real-IP is performed by the **RealIp plugin** or the **global `spec.realIp` config**; the result is written into `remote_addr`.
`IpSource::ClientIp` only **consumes** the already-processed `remote_addr`; it does not parse any header itself.

The doc comment was corrected on 2026-04-22 (`ip_restriction.rs`).

## Configuration Scenario Analysis

| Scenario | Content of `remote_addr` | Can XFF be spoofed |
|------|------------------|---------------|
| RealIp not configured | TCP IP (`client_addr`) | ❌ Ineffective; XFF is not read |
| RealIp plugin configured | RealIp extraction result | Depends on whether `trustedIps` is configured precisely |
| Global `spec.realIp` configured | Global RealIp extraction result | Same as above |

When the operator wants "based on real client IP", the correct approach is to explicitly configure RealIp, not to change the plugin default behavior.

## Fix Suggestions Not Accepted

- "When RealIp is not configured, automatically fall back to TCP IP" — **this is already the behavior**; no extra change needed
- "Force the docs to state that ClientIp must be paired with RealIp" — the doc comment has been corrected; the documentation does not need to repeatedly emphasize a wrong premise
- "Add a RemoteAddr key option for rate limiting" — the `RemoteAddr` option already exists (`IpSource::RemoteAddr`)

## The `KeyGet::Header` Case

`KeyGet::Header` depending on a spoofable request header is a **known design tradeoff**, consistent with rate-limiting behavior in Nginx/Envoy/Kong.
When the operator chooses Header as the rate-limit key, they must evaluate upstream trust themselves (e.g., a `X-Real-IP` injected fixedly by the CDN).
This is not a security vulnerability; it is a configuration tradeoff that the documentation explains.
