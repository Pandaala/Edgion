---
name: http1-01-duplicate-host-rejected
description: RFC 9110 §7.2 — HTTP/1.1 duplicate Host header rejected with 400; gated by SecurityProtectConfig.rejectDuplicateHost (default true). Reject re-reports of "accept and pick first / last Host".
---

# http1-01 — HTTP/1.1 duplicate Host header rejected with 400

**Status:** ✅ fixed · 2026-05-17
**RFC:** RFC 9110 §7.2 (MUST)

## Conclusion

Edgion rejects HTTP/1.1 requests carrying more than one `Host:` header line with `400 Bad Request`. Behavior is gated by `EdgionGatewayConfig.spec.securityProtect.rejectDuplicateHost` (default `true`); operators may flip to `false` only as a transitional escape hatch for legacy middleware.

Suggestions of the form "accept and pick the first/last Host", "downgrade to `tracing::warn!`", or "remove the operator opt-out" are **not accepted**.

## RFC clause

> "A server MUST respond with a 400 (Bad Request) status code to any HTTP/1.1
> request message that … contains more than one Host header field line …"

## Code shape today

- `pg_request_filter.rs::build_request_metadata` — counts `req_header.headers.get_all("host")` before path/ACME/hostname extraction; `>1` ⇒ `add_error(EdgionStatus::DuplicateHostHeader) + end_response_400`.
- `edgion_gateway_config.rs::SecurityProtectConfig.reject_duplicate_host: bool` — default `true` (`default_reject_duplicate_host`).
- `edgion_status.rs::EdgionStatus::DuplicateHostHeader = 400_05` — exhaustive `expected_http()` arm + test array updated.
- Integration: `examples/code/client/suites/http_route/filters/security.rs::test_duplicate_host_headers` (raw TCP, two `Host:` lines, asserts 400).

## Why this shape

1. **RFC 9110 §7.2 MUST + Envoy parity.** Envoy reaches the same 400 via inline-header coalescing into `:authority` + `authorityIsValid` (comma rejection) — no runtime override. Edgion arrives at the same observable outcome via an explicit per-header count.
2. **Default-on, opt-out via config.** Mirrors `requireSniHostMatch` in the same struct: safe out of the box, operator escape hatch for migration.
3. **HTTP/2 / gRPC-over-h2 untouched.** h2 stores authority in `:authority`; `get_all("host")` is normally empty there so the branch is a no-op.
4. **Hot-path zero cost on legitimate traffic.** Single `count() > 1` check before any allocating work.

## Re-evaluation triggers

- Pingora itself enforces duplicate-Host rejection at codec level (Edgion gate then becomes redundant).
- A new HTTP/3 ingress path is added; verify equivalent `:authority` repetition semantics.
- Operator-reported legitimate traffic that requires duplicate `Host` lines for reasons other than legacy-middleware migration.

## Reference

- Verifier: `tasks/todo/rfc-review/_raw/verification-a1.md → V2` (CONFIRM)
- Closure: [CLOSED-FINDINGS.md → http1-01](../CLOSED-FINDINGS.md)
- Operator docs: `docs/en/ops-guide/configuration/edgion-gateway-config.md` → `spec.securityProtect.rejectDuplicateHost`
- Industry reference: Envoy `source/common/http/header_utility.cc::authorityIsValid` (single-value authority enforcement)
