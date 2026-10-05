---
name: review-ssrf
description: Review knowledge for ssrf, grouped by decision type.
---

# ssrf review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Mandatory design

- [Upstream Address Safety](mandatory-design/upstream-address-safety.md)

## Regression guards

- [HTTP DNS endpoint egress and bootstrap policy](regression-guards/httpdns-endpoint-egress.md)

- [DSL `http.call` shared client disables env proxy — FIXED (was FB-01)](regression-guards/dsl-http-call-no-env-proxy.md)
- [DSL `http.call` re-checks `block_private_ips` on every redirect hop — FIXED (F-012)](regression-guards/dsl-http-call-redirect-per-hop.md)
- [Dsl ssrf guard naming](regression-guards/dsl-ssrf-guard-naming.md)
- [ForwardAuth / Webhook SSRF — triage rule](regression-guards/forward-auth-webhook-ssrf.md)
- [IPv4-mapped IPv6 SSRF-guard bypass in classify_backend_addr](regression-guards/ipv4-mapped-ipv6.md)
- [Wasm callout SSRF + header model — GUARDED + accepted tradeoffs](regression-guards/wasm-callout-egress-and-header-model.md)

## Superseded decisions

- [DSL `http.call` IP-literal-only SSRF tradeoff — SUPERSEDED](superseded/dsl-http-call-ip-literal-only.md)
