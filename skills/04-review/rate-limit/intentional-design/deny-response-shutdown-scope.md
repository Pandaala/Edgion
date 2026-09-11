---
name: deny-response-shutdown-scope
description: Use when reviewing findings that a plugin self-writes a deny/error response and never calls session.shutdown(); decides which plugin classes must shut the connection down and why the rate-limit plugins deliberately do not.
status: fixed
finding_id: gw-plugin-acl-rl-02
closed: 2026-08-03
knowledge_type: intentional-design
---

# Deny-Response `shutdown()` Scope — ACL Plugins Shut Down, Rate-Limit Plugins Deliberately Do Not

## Conclusion

`RequestRestriction` was fixed to route both self-written responses (the config-error 500 and
the policy-deny) through `auth_common::send_gateway_plain_error_response`, which ends with
`session.shutdown()`. `RateLimitLocal` and `RateLimitRedis` keep writing their rejection responses
inline **without** `shutdown()` — that is a deliberate choice, not an oversight.

Fix suggestions of the form "`RateLimitLocal`/`RateLimitRedis` also hand-roll
`write_response_header` → `write_response_body` → `return ErrTerminateRequest` without
`shutdown()`, make them match the documented standard sequence" are **not accepted**.

## Core Rationale

**1. The rule is by plugin class, not by "who writes their own response"**

Access-control denials (`IpRestriction`, the former `GlobalAccessControl`, `RequestRestriction`) express a
*persistent* verdict about the client: the same connection would be denied on every subsequent
request. Tearing the HTTP/1.x connection down is consistent with that verdict and matches the
`plugins-http-F014` precedent, which routed `ip_restriction` and `ldap_auth` through the shared
helper for exactly this reason.

A 429 is the opposite: an explicitly *temporary* condition carrying `Retry-After`. Closing the
connection on every rejection forces the client into a full TCP (and, on TLS listeners, full
handshake) round trip per retry, adding gateway-side handshake cost precisely when the plugin
exists to shed load. Kong and APISIX rate-limit plugins likewise return 429 on a keep-alive
connection rather than closing it.

**2. Not calling `shutdown()` produces a correctly framed, reusable connection**

The rejection responses carry no `Content-Length`, so Pingora's `init_body_writer_comm` selects
chunked framing on HTTP/1.1 and `write_response_body(_, true)` terminates it. Keep-alive stays
intact, the client sees a complete response, and no `close_delimited` fallback is triggered
(`pingora-core` `protocols/http/v1/server.rs`). There is no hang, no truncation, and no security
consequence — the only difference is connection reuse.

**3. The `ErrTerminateRequest` contract does not imply connection teardown**

`PluginRunningResult::ErrTerminateRequest` means "the response is already written, skip the
upstream". Several shipping paths return it — or write a response and continue — without any
`shutdown()`: the CORS 204 preflight short-circuit (`cors/plugin.rs`), `request_redirect`, and
`extension_ref`. `shutdown()` is a property of the *deny semantics*, not of self-writing a
response.

## Fix Suggestions Not Accepted

- Add `session.shutdown().await` to `RateLimitLocal::reject_request` — turns every 429 into a
  connection teardown, amplifying handshake cost under exactly the load the plugin sheds.
- Add `session.shutdown().await` to `RateLimitRedis::reject_request` — same, and it additionally
  hits the `OnRedisFailure::Deny` fail-close path, where a Redis outage would then churn every
  downstream connection.
- Route the rate-limit rejections through `send_gateway_plain_error_response` "for consistency" —
  the helper's signature is `(status, content_type, body)` and cannot carry `Retry-After` or the
  `X-RateLimit-*` set; extending it for that is rejected separately by
  [header-injection-duplication.md](header-injection-duplication.md).
- Treat "plugin self-writes a response without `shutdown()`" as a defect pattern on its own —
  see rationale 3.
- Argue that calling `shutdown()` is unsafe because it abruptly closes HTTP/2 connections — that
  premise is false and was closed as `gw-plugin-acl-rl-01`; see
  [../h2-grpc/plugin-session-shutdown-h2-no-gate.md](../../h2-grpc/review-evidence/plugin-session-shutdown-h2-no-gate.md).
  The rate-limit exemption rests on 429 semantics alone, not on any H2 risk.

## Re-evaluation Triggers

Re-open this decision only if:

- Rate limiting gains a persistent-ban mode (e.g. an escalating block after N violations), which
  would make the verdict persistent and align it with the ACL class.
- Measurements show connection reuse after a 429 lets a single client hold a slot in a way the
  connection limits do not already bound.

## Reference Cases

- `plugins-http-F014` (2026-05-26) — established the shared helper and fixed the
  `ip_restriction` / `ldap_auth` omission; scoped to the ACL/auth class only.
- `gw-plugin-acl-rl-01` (2026-08-03, ❌ false positive) — closed the claim that
  `PingoraSessionAdapter::shutdown()` abruptly TCP-closes plain HTTP/2 streams. Adding a
  `shutdown()` caller carries no HTTP/2 cost: the H2 arm only sends `RST_STREAM`, and only when
  the stream has not already ended, so the call is a no-op after `write_response_body(_, true)`.
  See [../h2-grpc/plugin-session-shutdown-h2-no-gate.md](../../h2-grpc/review-evidence/plugin-session-shutdown-h2-no-gate.md).
- `edgion-gateway/src/plugins/http/common/auth_common.rs` — `send_gateway_plain_error_response`,
  the canonical `write_response_header` → `write_response_body` → `shutdown` sequence.
- `edgion-gateway/src/runtime/server/error_response.rs` — `shutdown_after_error_response`, the
  `!is_http2()` gate and its rationale.

## Historical resolution evidence: plugins-http-F014

`ldap_auth::service_unavailable` (503) and `ip_restriction` deny block (configurable 4xx + JSON body) hand-coded the `write_response_header` → `write_response_body` → `shutdown` sequence, diverging from the shared `auth_common::send_gateway_auth_error_response` helper (which is auth-challenge-only because it requires `WWW-Authenticate`). Most concrete gap: `ip_restriction` never called `session.shutdown()`. Fixed by adding a sibling `send_gateway_plain_error_response(session, status, content_type, body)` to `common/auth_common.rs` and routing both plugins through it. `ip_restriction` now shuts the connection down (matching the helper's invariant); the previous `ResponseHeader::build(status).unwrap_or_else(|_| build(403))` fallback was dead because `IpRestrictionConfig::validate_and_init` enforces `100 ≤ status < 600` at config time. Re-reports of "plugin error path hand-codes write_response_header → write_response_body → shutdown" should route to the new helper rather than duplicating the sequence

## Resolution references

- `plugins-http-F014` — 2026-05-26.
- `gw-plugin-acl-rl-02` — 2026-08-03 (partial fix + ⚠️ accepted tradeoff).
