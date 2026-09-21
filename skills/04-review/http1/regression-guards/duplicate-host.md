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

- `conf_handler_impl.rs::ResolvedGatewayConfig` — resolves the switch into the immutable EGC runtime Arc with a safe `true` fallback for an absent, missing, or deleted configuration.
- `pg_new_ctx.rs` + `pg_request_filter.rs::build_request_metadata` — each ordinary HTTP request captures the switch with the request-body, RemoteIp-header, and preflight policies from one publication generation. The filter generates `x_trace_id` first (so the rejection's access-log entry is correlatable with `tls.log`), then counts `req_header.headers.get_all("host")` before path/ACME/hostname extraction; `>1` ⇒ `record_error(EdgionErrorCode::DuplicateHostHeader) + end_response_400`.
- `redirect_http.rs::EdgionHttpRedirectProxy::{new_ctx,request_filter}` — the redirect listener retains only the referenced EGC name; each new context captures the current switch, and the request filter applies the same gate before it can echo a Host into the 301 `Location:` URL.
- `edgion_gateway_config.rs::SecurityProtectConfig.reject_duplicate_host: bool` — default `true` (`default_reject_duplicate_host`).
- `diagnostic.rs::EdgionErrorCode::DuplicateHostHeader` — stable string code emitted with intrinsic `kind: "error"`; HTTP 400 is selected explicitly by the response path.
- Static integration: `edgion-tests/src/client/suites/http_route/filters/security.rs` — `test_duplicate_host_headers` (same-case `Host:` × 2) and `test_duplicate_host_headers_mixed_case` (`Host:` + `host:`); both raw TCP, both assert HTTP/1.1 400.
- Hot-reload integration: `edgion-tests/src/client/dynamic/scenarios/duplicate_host_policy_hot_reload.rs` — `DuplicateHostPolicyHotReload` keeps ordinary and redirect listeners running while the referenced EGC moves through default `true`, explicit `false`, restored `true`, and deletion fallback. Raw HTTP/1.1 requests retain both `Host` field lines; the escape-hatch generation asserts ordinary routing (200) and redirect behavior (301 plus `Location`), while every safe generation asserts 400 on both listeners. The focused cross-process scenario passed on 2026-09-21.

### Test coverage gaps (intentional)

- **gRPC-over-h1 → `grpc-status=3`**: `end_response_400` is verified by code inspection to call `send_grpc_error_three_frame(..., 400)` (maps to `INVALID_ARGUMENT`), but raw-TCP wire assertions for HTTP/1.1 trailers are too brittle to add here — pingora's `write_response_trailers` is designed for HTTP/2 framing. An h2-native scenario would be more reliable if real test coverage of this path is needed.

## Known limitation

`Host: a, b` (single line, comma-separated multi-value) is **not** detected by `count() > 1`. RFC 9110 §7.2 literally targets "more than one Host header field line" — the multi-value-in-one-line case falls outside that clause. In practice the merged value is treated as a single literal hostname and fails route lookup (404 RouteNotFound), so it does not constitute a smuggling primitive. Fix suggestions that propose splitting the single-line value on `,` to re-trigger the check are **not accepted** (would mis-classify legitimate IPv6 literals and is outside RFC §7.2's scope).

## Why this shape

1. **RFC 9110 §7.2 MUST + Envoy parity.** Envoy reaches the same 400 via inline-header coalescing into `:authority` + `authorityIsValid` (comma rejection) — no runtime override. Edgion arrives at the same observable outcome via an explicit per-header count.
2. **Default-on, opt-out via config.** Mirrors `requireSniHostMatch` in the same struct: safe out of the box, operator escape hatch for migration. EGC updates affect newly created ordinary and redirect request contexts without rebinding listeners; in-flight requests retain their captured generation. Deletion, a missing reference, and no reference all restore `true`.
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
- `D0013` — 2026-09-21; added cross-process duplicate-Host policy hot-reload coverage for ordinary and redirect listeners.
