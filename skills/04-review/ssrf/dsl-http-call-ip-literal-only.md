---
name: dsl-http-call-ip-literal-only
description: Historical record for the former DSL `http.call` IP-literal-only SSRF tradeoff, superseded by dial-time `EgressResolver` enforcement.
---

# DSL `http.call` IP-literal-only SSRF tradeoff — SUPERSEDED

## Conclusion

This 2026-05-26 tradeoff no longer describes the implementation. As of 2026-07-29,
guarded DSL clients use the canonical `EgressResolver`. Direct IP literals still use
`is_ssrf_blocked_ip_literal` as an early check, while hostnames are classified after
resolution against the exact addresses reqwest receives for dialing.

The resolver closes the former DNS-rebinding window without a separate preflight lookup:
reqwest dials the resolver's checked addresses rather than resolving the hostname again.
Redirected `Str` and `Nil` requests receive the same per-connection check; opaque `Body`
requests do not follow redirects.

## Retained Decisions

- `httpBlockPrivateIps` remains default-off because calling internal services is a
  supported DSL use case.
- Proxy environment pickup remains disabled.
- NetworkPolicy remains recommended as defense in depth.
- Request-derived resolver rejections increment the canonical egress metric but do not
  emit one WARN per address, preventing client-driven log amplification.

## Reference Cases

- F-018 (2026-05-26) — originally closed as an accepted IP-literal-only tradeoff;
  superseded on 2026-07-29.
- F-012 (2026-05-26) — redirect literals remain checked by the custom redirect policy;
  hostname results are additionally checked by `EgressResolver`.
- `skills/01-architecture/02-gateway/dsl-engine-security-model.md` §7 — maintainer-view rationale.
- [forward-auth-webhook-ssrf.md](forward-auth-webhook-ssrf.md) — same host-string-blocklist reasoning for ForwardAuth / Webhook.
- [upstream-address-safety.md](upstream-address-safety.md) — the stricter post-DNS-resolution standard that applies to **upstream dial paths** (DynamicExternalUpstream, RegionRoute, etc.), a different threat surface than the trusted-author `http.call` builtin.
