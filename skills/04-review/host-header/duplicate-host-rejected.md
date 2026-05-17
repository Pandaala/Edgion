---
name: duplicate-host-rejected
description: Use when reviewing host smuggling / duplicate Host header / RFC 9110 §7.2 findings on the HTTP/1.1 ingress path; covers the verdict and the gated escape hatch.
---

# Duplicate HTTP/1.1 Host Header Rejection — FIXED

## Conclusion

Edgion rejects HTTP/1.1 requests carrying more than one `Host` header line with `400 Bad Request`. The check lives in `build_request_metadata` (gateway HTTP request filter) and is gated by `EdgionGatewayConfig.spec.securityProtect.rejectDuplicateHost` (default `true`).

Fix suggestions of the form "also accept and pick the first / last `Host:` value", "make this `tracing::warn!` instead of `400`", or "remove the operator opt-out switch" are **not accepted**.

## Core Rationale

**1. RFC 9110 §7.2 is a hard MUST**

> "A server MUST respond with a 400 (Bad Request) status code to any HTTP/1.1 request message that … contains more than one Host header field line …"

Silently keeping the first value is the host smuggling primitive: the gateway routes to `victim.internal.example` while a downstream cache / proxy interprets `attacker.example`, crossing per-host security and tenancy boundaries.

**2. Aligns with Envoy default behavior**

Envoy stores `Host` as the `:authority` inline header. Multiple `Host` lines coalesce with comma separators; the resulting value then fails `HeaderUtility::authorityIsValid` (comma is not a valid authority character) and Envoy returns `400 InvalidAuthority`. Envoy ships this default with no runtime override. Edgion arrives at the same observable outcome via an explicit per-header count.

**3. Operator escape hatch, default safe**

`rejectDuplicateHost` defaults to `true` (RFC- and Envoy-aligned). Operators bridging legacy middleware that emits duplicate `Host` lines can flip it to `false` on a single `EdgionGatewayConfig`. The toggle lives next to `requireSniHostMatch` / `xForwardedForLimit`, matching the existing `SecurityProtectConfig` ergonomics.

**4. Zero cost on the legitimate path / HTTP/2 untouched**

`req_header.headers.get_all("host").iter().count()` is constant work on every request; legitimate clients always have count ≤ 1. HTTP/2 carries authority in the `:authority` pseudo-header and `get_all("host")` is normally empty, so h2 and gRPC-over-h2 are unaffected.

## Fix Suggestions Not Accepted

- "Pick the first Host value, log a warning, keep routing" — that *is* the host smuggling vulnerability the MUST exists to prevent.
- "Make this check fire only for non-gRPC requests" — RFC 9110 §7.2 does not carve out gRPC; gRPC-over-h1 also has the same smuggling surface.
- "Remove the operator opt-out (`rejectDuplicateHost: false`)" — the escape hatch is a stated migration path; removal needs deprecation, not casual deletion.
- "Move the check to a plugin" — the host value drives core route lookup; it must run before plugins.

## Re-evaluation Triggers

Re-open this decision only if:

- Pingora itself starts enforcing the rejection at the codec level (in which case the Edgion-side gate becomes redundant and the toggle / `EdgionStatus::DuplicateHostHeader` variant can be retired).
- A new HTTP/3 entry path is added; verify the equivalent semantics on `:authority` repetition.
- An operator-reported incident demonstrates legitimate traffic that must send duplicate `Host` lines for reasons other than legacy middleware migration.

## Reference Cases

- Original finding: `tasks/todo/rfc-review/rfc-violation-http1-01-duplicate-host-not-rejected.md` (closed)
- Verifier: `tasks/todo/rfc-review/_raw/verification-a1.md` → V2 (CONFIRM)
- Implementation: `src/core/gateway/routes/http/proxy_http/pg_request_filter.rs` (`build_request_metadata`)
- Config schema: `src/types/resources/edgion_gateway_config.rs` (`SecurityProtectConfig.reject_duplicate_host`)
- Status code: `src/types/edgion_status.rs` (`EdgionStatus::DuplicateHostHeader = 400_05`)
- Integration test: `examples/code/client/suites/http_route/filters/security.rs` (`test_duplicate_host_headers`)
- Operator docs: `docs/en/ops-guide/configuration/edgion-gateway-config.md` → `spec.securityProtect.rejectDuplicateHost`
