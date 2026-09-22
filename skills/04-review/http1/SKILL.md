---
name: review-http1
description: Review knowledge for http1, grouped by decision type.
---

# http1 review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [Downstream HTTP/1 keepalive defaults to 10,000 requests per connection](intentional-design/downstream-keepalive-request-limit-default.md)
- [rfc-003 — `Max-Forwards` not honored for TRACE / OPTIONS](intentional-design/max-forwards-not-honored.md)
- [http1-02 — No manual `Connection: close` injection in proxied responses](intentional-design/no-connection-close-injection.md)

## Compatibility constraints

- [Response compression requires a known response length](compatibility-constraints/compression-requires-known-response-length.md)
- [Response-body sink extras and transform order](compatibility-constraints/response-body-sink-transform-order.md)

## Regression guards

- [append_request_header drops additional same-name request header values (F-001)](regression-guards/append-request-header-multi-value.md)
- [Header EOS Drives the Terminal Response-Body Hook](regression-guards/bodyless-header-eos-terminal-body-hook.md)
- [Custom Upstream Reuse Is an Independent Decision](regression-guards/custom-upstream-reuse-is-independent.md)
- [Downstream HTTP/1 keepalive limits count total requests, not later reuses](regression-guards/downstream-keepalive-request-limit-counting.md)
- [http1-01 — HTTP/1.1 duplicate Host header rejected with 400](regression-guards/duplicate-host.md)
- [Suppressed Follow-Up Tasks Drain Their Sink Extras](regression-guards/gw-16-suppressed-sink-extras-drained.md)
- [http1-03 — Hop-by-hop filtering at plugin sub-requests and the main proxied response](regression-guards/hop-by-hop-dynamic-stripping.md)
- [Outbound `forwardAll` preserves duplicate request headers (gw-plugin-outbound-02) — FIXED](regression-guards/outbound-forward-all-preserves-duplicate-headers.md)
- [WASM local responses collapse duplicate response headers — FIXED](regression-guards/wasm-local-response-duplicate-headers.md)
- [proxy-wasm `proxy_set_header_map_pairs` whole-map replace drops omitted headers and preserves duplicates (runtime-03) — FIXED](regression-guards/wasm-set-header-map-pairs-replace-all.md)

## Superseded decisions

- [Pingora Streamed Transfer Codings — SUPERSEDED BY COMMON INGRESS POLICY](superseded/streamed-transfer-encoding-preserves-undecoded-codings.md)
