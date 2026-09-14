---
name: request-local-reply-final-status-validation
description: Use when reviewing HTTP request-plugin status fields that produce a local final response.
knowledge_type: regression-guards
---

# Request local replies require a final HTTP status

## Conclusion and scope

Every request-stage plugin status field that produces a local final response must
use `edgion_resources::http_status::is_valid_final_http_status` for runtime
validation and expose the same `200..=599` bounds in its generated schema. This
applies to:

- `FaultInjection.abort.statusCode`
- `Mock.statusCode`
- `RequestAccessPolicy.status`
- `RateLimitLocal.rejectStatus`
- `RateLimitRedis.rejectStatus`
- `IpRestriction.status`
- `RequestRestriction.status`

Informational `1xx` values are not valid final local replies. This rule does not
apply to fields that inspect or match upstream response statuses without ending
the current request.

## Rationale and evidence

`edgion-resources/src/http_status.rs` owns the canonical final-status predicate,
which accepts only `200..=599`. The Gateway's
`PingoraSessionAdapter::write_response_header` claims a final response through
the request context using that same predicate, so accepting `1xx` in plugin
configuration cannot produce the configured terminal response.

The seven plugin configurations previously used independent `100..600` checks.
Their validation and schema bounds now share the canonical final-status range.
`EdgionPlugins::validate_plugins` is the Controller-side preparse path, while
`compile_plugins` consumes the same configuration validation on the Gateway;
the regression test compares their results for every affected plugin.

## Alternatives and limits

The fix intentionally rejects `100`, `101`, and `199` during configuration
validation instead of adding special runtime handling for informational
responses. A request plugin's local reply is terminal, so permitting an interim
response would contradict both its configuration contract and the canonical
final-response claim.

Do not mechanically apply this range to health checks, retry matchers, or other
fields whose purpose is to observe an upstream status rather than author a final
local response.

## Verification and re-evaluation

Targeted resource tests cover invalid values `100`, `101`, `199`, and `600`,
valid boundary values `200` and `599`, and generated schema bounds. The Gateway
parity test
`request_local_reply_status_validation_matches_controller_and_gateway` verifies
that Controller validation and Gateway compilation agree for all seven plugin
types.

Re-evaluate this rule if the canonical final-status predicate changes, if Pingora
gains an explicit informational-response API used by request plugins, or if a
listed field stops representing a terminal local reply.

## Resolution references

- `use-canonical-final-http-status-validation-for-local-replies` — fixed on
  2026-09-14; original finding in
  `tasks/todo/use-canonical-final-http-status-validation-for-local-replies.md`.
