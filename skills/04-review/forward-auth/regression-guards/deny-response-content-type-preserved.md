---
name: deny-response-content-type-preserved
description: Use when reviewing whether ForwardAuth labels every relayed deny body as JSON or fails to preserve the auth service response Content-Type.
knowledge_type: regression-guards
---

# ForwardAuth Deny Response Content-Type

**Status / verdict**: fixed (`gw-plugin-auth-b-01`, 2026-08-03).

ForwardAuth relays the auth service's bounded response body on a denied request. The
body is not plugin-authored, so its media type must come from the same auth response.
The deny path now copies the auth service `Content-Type` when it is present and valid;
when it is absent, the gateway leaves it absent instead of guessing. A received
`Connection` nomination suppresses copying that field, including automatic
`Content-Type`; a non-text `Connection` value suppresses all remote header copying. The separate
transport-failure path still labels its plugin-owned JSON body as `application/json`.

`decision.clientHeaders` remains the operator-selected mechanism for other response
headers. Operators no longer need to list `Content-Type` there to correct the default
deny response.

## Regression coverage

- `deny_copies_client_headers_and_shortcircuits` verifies that a denied
  `text/plain; charset=utf-8` response keeps that media type.
- `transport_fail_records_fail_close_before_response_write` verifies that the
  plugin-owned unavailable response remains `application/json`.

## Re-report guard

Before reporting this issue again, verify both call sites of
`ForwardAuth::send_error_response`: the auth-service deny call must pass the received
media type, while the transport-failure call must explicitly pass
`application/json`.

## Resolution references

- `gw-plugin-auth-b-01` — 2026-08-03.

## Shared response policy

`decision.clientHeaders` validates names against the [shared response policy](../../plugin-runtime/regression-guards/response-header-mutation-policy.md).
All received `Connection` field lines participate in nomination filtering.
Allowed selected duplicate values, including `Set-Cookie` and
`WWW-Authenticate`, are copied without collapsing to the last value. The bounded
denial body remains independent of header-copy suppression.

- `unify-response-header-mutation-policy` — scoped Content-Type preservation and repeated-header forwarding; current verification is recorded in the shared policy page.
