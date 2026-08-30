---
name: ip-source-client-ip-false-positive
description: Decision rule for IP restriction / rate limiting RemoteIp findings. Read this file first when reviewing "RemoteIp can be spoofed via XFF when RealIp is not configured" findings.
---

# IpSource::RemoteIp / KeyGet::RemoteIp — NOT-AN-ISSUE

## Conclusion

Findings of the form "without the RealIp plugin enabled, an attacker can bypass the IP allowlist/rate limit by spoofing `X-Forwarded-For`" rest on a wrong premise; classified as NOT-AN-ISSUE.

## Code Path (key evidence)

```
IpSource::RemoteIp  → session.remote_ip() → ctx.request_info.remote_ip
KeyGet::RemoteIp    → self.remote_ip()    → ctx.request_info.remote_ip
```

`remote_ip` is initialized in `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs`:

```rust
ctx.request_info.remote_ip = if let Some(extractor) = &edgion_http.real_ip_extractor {
    extractor.extract_real_ip(&downstream_peer_ip_str, req_header)
} else {
    // No extractor configured, use the effective downstream peer
    ctx.request_info.downstream_peer_ip.clone()
};
```

**When `real_ip_extractor` is not configured, `remote_ip` is set directly to the effective downstream peer; XFF/X-Real-IP request headers are not read at all.**

## Why the doc comment looked like "reads XFF"

The original doc comment for `IpSource::RemoteIp` read "Extract from X-Forwarded-For (first IP) or fallback to X-Real-IP", which is misleading.
In reality, reading XFF/X-Real-IP is performed by the **RealIp plugin** or the **global `spec.realIp` config**; the result is written into `remote_ip`.
`IpSource::RemoteIp` only **consumes** the already-processed `remote_ip`; it does not parse any header itself.

The doc comment was corrected on 2026-04-22 (`ip_restriction.rs`).

## Configuration Scenario Analysis

| Scenario | Content of `remote_ip` | Can XFF be spoofed |
|------|------------------|---------------|
| RealIp not configured | Effective downstream peer | ❌ Ineffective; XFF is not read |
| RealIp plugin configured | RealIp extraction result | Depends on whether `trustedIps` is configured precisely |
| Global `spec.realIp` configured | Global RealIp extraction result | Same as above |

When the operator wants "based on real client IP", the correct approach is to explicitly configure RealIp, not to change the plugin default behavior.

## Fix Suggestions Not Accepted

- "When RealIp is not configured, automatically fall back to TCP IP" — **this is already the behavior**; no extra change needed
- "Force the docs to state that RemoteIp must be paired with RealIp" — the doc comment has been corrected; the documentation does not need to repeatedly emphasize a wrong premise
- "Add a DirectPeerIp key option for rate limiting" — generic KeyGet intentionally exposes only the resolved `RemoteIp` source

## The `KeyGet::Header` Case

`KeyGet::Header` depending on a spoofable request header is a **known design tradeoff**, consistent with rate-limiting behavior in Nginx/Envoy/Kong.
When the operator chooses Header as the rate-limit key, they must evaluate upstream trust themselves (e.g., a `X-Real-IP` injected fixedly by the CDN).
This is not a security vulnerability; it is a configuration tradeoff that the documentation explains.
