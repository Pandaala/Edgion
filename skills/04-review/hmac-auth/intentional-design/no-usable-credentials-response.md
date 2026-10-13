---
name: no-usable-credentials-response
description: Use when reviewing standalone HmacAuth responses with absent or empty resolved credentials.
knowledge_type: intentional-design
---

# Standalone HMAC — fixed generic 500 when no usable credentials exist

## Conclusion and scope

Standalone `HmacAuth::run_request` deliberately retains HTTP 500 for
`HmacVerifyFailure::NoCredentials`: an Authorization or Proxy-Authorization header
is present, but the immutable resolved credential map is absent or empty. The
public body is fixed `500 Internal gateway error`; the HMAC challenge retains the
configured realm. This is an explicit exception to the normal generic-401
auth-Secret policy in [design rules section F](../../../00-design-rules.md#f-failure-policy--error-replies).

The user accepted retaining 500 and closing `ARCH-036` on 2026-10-13 after
considering both classifications and the historical policy conflict. This
acceptance revises the HMAC-specific status policy; it does not establish that
every server-side authentication configuration failure should return 500.

## Rationale and evidence

The chosen classification treats an unavailable server credential set as a
server-side failure, while ordinary invalid client credentials remain 401.
`HmacVerifier::verify_headers_with` checks credential availability after checking
for the authorization header, before parsing that header or capturing the body.
`HmacAuth::run_request` records private `push_err("no creds configured")`, applies
shared `apply_auth_failure_delay`, and calls canonical
`send_gateway_auth_error_response` with status 500 and no public error code.
The helper writes the challenge and generic body, then shuts down the exchange;
the plugin terminates fail closed. No credential contents, group attribution,
Secret reference or provider detail belong in this response or failure log.

Historical B0081, commit `2ac3cc2b9` (2026-09-01, `Tighten HmacAuth runtime
ownership`), explicitly preserved 500 while replacing `No credentials configured`
with `Internal gateway error`. Its independent reviews approved the bounded
ownership and response change. The generic-401 rule already existed, so that
historical approval alone did not authorize an exception. The 2026-10-13 user
decision and corresponding canonical-rule update resolve that conflict without
changing B0081's ownership, privacy or response-order boundaries.

## Alternatives and limits

Using generic 401 would unify unavailable credentials with client authentication
denial and hide credential availability in the public status. That alternative
was declined for this branch. Retaining 500 preserves the chosen server-failure
classification, with the accepted cost that clients can distinguish unavailable
server credentials from ordinary denial. Client retry policies and monitoring
may treat this 5xx differently and may retry a persistent configuration failure.
No throughput or memory benefit is claimed.

Other authentication plugins retain the generic-401 policy. Missing-token
anonymous handling, ordinary HMAC denials, shared HMAC condition boolean outcomes,
configuration validation, credential resolution/replacement, cheap authentication
before body IO, signed-body digest policy and mechanical body-error statuses
remain unchanged. This decision does not cover the separate internal
`credential disappeared` invariant branch or permit fail-open behavior.

## Verification and re-evaluation

Static source inspection on 2026-10-13 checked
`edgion-gateway/src/plugins/http/hmac_auth/plugin.rs`, the shared helper in
`plugins/http/common/auth_common.rs`, and
`edgion-gateway/tests/unit/plugins/http/hmac_auth/plugin_test.rs`.
The existing `no_credentials_uses_generic_exact_internal_response` test asserts
500, exact body/challenge, one response/shutdown sequence and the private
diagnostic. It uses direct default-config construction; it is source evidence,
not fresh execution or coverage of the valid Secret-group resolution path.
No fresh runtime tests or builds were run for this documentation-only decision.
B0081's test results remain historical evidence.

Reconsider if the product chooses a uniform authentication-disclosure policy,
client retry or alert behavior makes this distinction unacceptable, the response
begins exposing credential/provider detail, or verifier/runtime changes broaden
`NoCredentials` beyond an absent or empty resolved map with a present token.

## Resolution references

- `ARCH-036` — accepted design on 2026-10-13; proposed 401 correction declined.
- B0081 — historical record at `git show 2ac3cc2b9:tasks/pending/edgion-gateway-file-optimization/batches/B0081-hmac-auth-bounded-runtime-ownership.md`.
