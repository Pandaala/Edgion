---
name: grpc-empty-authority-rejected
description: RFC 9110 §7.2 + RFC 9113 §8.3.1 — empty / whitespace Host / :authority is rejected for gRPC too (no carve-out). Reject re-reports proposing to skip the empty-hostname guard on the gRPC path.
knowledge_type: regression-guards
---

# http1-05 — Empty Host / :authority rejected for gRPC

**Status:** ✅ fixed · 2026-05-17
**RFC:** RFC 9110 §7.2 (MUST) · RFC 9113 §8.3.1 (HTTP/2 :authority parity)

## Conclusion

After hostname normalization (`NormalizedHostname::new` trims whitespace, strips port, lowercases), an empty `Host` / `:authority` value is rejected with `400 Bad Request` regardless of whether the request is HTTP or gRPC. The gRPC code path takes the same guard; `end_response_400` already maps gRPC requests to `grpc-status 3` (INVALID_ARGUMENT) trailers-only.

Suggestions of the form "skip the empty-hostname check for gRPC because gRPC handles routing differently", "match catch-all GRPCRoute when authority is absent", or "downgrade rejection to a `tracing::warn!`" are **not accepted**.

## RFC clause

> "A server MUST respond with a 400 (Bad Request) status code to any HTTP/1.1
>  request message that lacks a Host header field … or a Host header field
>  with an invalid field value." — RFC 9110 §7.2

HTTP/2 carries the equivalent in the `:authority` pseudo-header (RFC 9113 §8.3.1); the same validity requirement applies.

## Code shape today

- `pg_request_filter.rs::build_request_metadata` — after normalization, `if ctx.request_info.hostname.is_empty() { ctx.record_error(EdgionErrorCode::HostMissing); end_response_400(...) }`. No `is_grpc_request` carve-out.
- `error_response.rs::end_response_400` — `is_grpc_request` branch delegates to `send_grpc_error_three_frame(..., 400)`, which `http_to_grpc_status` maps to `(3, "invalid argument")`.
- `ctx.rs::NormalizedHostname::new` — single normalization site; downstream consumers rely on the invariant.
- Integration: covered indirectly by `test_empty_hostname` (HTTP/1.1 path) in `edgion-tests/src/client/suites/http_route/filters/security.rs`. A targeted gRPC scenario is **not** added: HTTP/1.1 + `Content-Type: application/grpc` exercises the gRPC error-response path (`send_grpc_error_three_frame` writes `HTTP/1.1 200` + `content-type: application/grpc` + `grpc-status: 3` trailers), and HTTP/1.1 trailers framing is too brittle to assert reliably from raw TCP. A real HTTP/2 gRPC client cannot construct a whitespace `:authority` without bypassing `h2`/`tonic` validation. The rule is held by this entry + verifier CONFIRM, not by a wire test.

## Why this shape

1. **RFC 9110 §7.2 MUST.** An empty / whitespace-only field value is "invalid"; the MUST applies regardless of transport protocol or content-type.
2. **Tenant isolation.** Without the guard, the gRPC matcher's `matches_hostname` returns `true` for any catch-all GRPCRoute (`hostnames.is_empty() || hostnames[0] == CATCH_ALL_HOSTNAME`). A whitespace `:authority` would land on the first catch-all route — cross-tenant for multi-tenant deployments.
3. **gRPC error mapping is already wired.** `end_response_400` covers gRPC; no new helper is required.
4. **Hot path zero cost.** One short-circuit removed; legitimate requests pay an `is_empty()` check that they paid before.

## Re-evaluation triggers

- Pingora's HTTP/2 codec rejects whitespace `:authority` at frame parse (Edgion guard then becomes redundant).
- A new HTTP/3 ingress path is added; verify equivalent `:authority` normalization and rejection.
- An operator-supplied reverse-proxy chain legitimately needs an empty `:authority` for a documented gRPC scenario.

## Reference

- Historical verifier (source queue removed): `tasks/todo/rfc-review/_raw/verification-a1.md → V7` (CONFIRM)
- Sibling rule: [../rfc-divergence/duplicate-host.md](../../http1/regression-guards/duplicate-host.md) (duplicate Host rejection, same RFC §7.2 family)
- Conformance: `skills/08-kubernetes/01-gateway-api/SKILL.md` (GATEWAY-GRPC ALL PASS, must remain so)

## Resolution references

- `http1-05` — 2026-05-17.
