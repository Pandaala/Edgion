---
name: duplicate-host-rejected
description: RFC 9110 §7.2 — HTTP/1.1 duplicate Host header rejected with 400; gated by SecurityProtectConfig.rejectDuplicateHost (default true). Reject re-reports of "accept and pick first / last Host".
knowledge_type: regression-guards
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

- `pg_request_filter.rs::build_request_metadata` — generates `x_trace_id` first (so the rejection's access-log entry is correlatable with `tls.log`), then counts `req_header.headers.get_all("host")` before path/ACME/hostname extraction; `>1` ⇒ `record_error(EdgionErrorCode::DuplicateHostHeader) + end_response_400`.
- `redirect_http.rs::EdgionHttpRedirectProxy::request_filter` — same gate at the top of the HTTP→HTTPS redirect listener; prevents an attacker from echoing a smuggled Host into the 301 `Location:` URL.
- `edgion_gateway_config.rs::SecurityProtectConfig.reject_duplicate_host: bool` — default `true` (`default_reject_duplicate_host`).
- `diagnostic.rs::EdgionErrorCode::DuplicateHostHeader` — stable string code emitted with intrinsic `kind: "error"`; HTTP 400 is selected explicitly by the response path.
- Integration: `edgion-tests/src/client/suites/http_route/filters/security.rs` — `test_duplicate_host_headers` (same-case `Host:` × 2) and `test_duplicate_host_headers_mixed_case` (`Host:` + `host:`); both raw TCP, both assert HTTP/1.1 400.

### Test coverage gaps (intentional)

- **Opt-out path** (`rejectDuplicateHost: false` → duplicates accepted): not covered as a wire test because it requires a separate suite with its own `EdgionGatewayConfig` YAML (the shared `HTTPRoute/Filters/Security` suite cannot toggle the flag mid-run). The unit-level `unwrap_or(true)` default path is exercised by the existing positive cases.
- **gRPC-over-h1 → `grpc-status=3`**: `end_response_400` is verified by code inspection to call `send_grpc_error_three_frame(..., 400)` (maps to `INVALID_ARGUMENT`), but raw-TCP wire assertions for HTTP/1.1 trailers are too brittle to add here — pingora's `write_response_trailers` is designed for HTTP/2 framing. An h2-native scenario would be more reliable if real test coverage of this path is needed.

## Known limitation

`Host: a, b` (single line, comma-separated multi-value) is **not** detected by `count() > 1`. RFC 9110 §7.2 literally targets "more than one Host header field line" — the multi-value-in-one-line case falls outside that clause. In practice the merged value is treated as a single literal hostname and fails route lookup (404 RouteNotFound), so it does not constitute a smuggling primitive. Fix suggestions that propose splitting the single-line value on `,` to re-trigger the check are **not accepted** (would mis-classify legitimate IPv6 literals and is outside RFC §7.2's scope).

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

- Historical verifier (source queue removed): `tasks/todo/rfc-review/_raw/verification-a1.md → V2` (CONFIRM)
- Operator docs: `docs/en/ops-guide/configuration/edgion-gateway-config.md` → `spec.securityProtect.rejectDuplicateHost`
- Industry reference: Envoy `source/common/http/header_utility.cc::authorityIsValid` (single-value authority enforcement)

## Resolution references

- `http1-01` — 2026-05-17.
