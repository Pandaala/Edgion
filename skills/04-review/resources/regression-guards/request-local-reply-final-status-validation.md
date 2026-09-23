---
name: request-local-reply-final-status-validation
description: Use when reviewing status fields and dynamic producers that create a terminal local HTTP response.
knowledge_type: regression-guards
---

# Terminal local replies require a final HTTP status

## Conclusion and scope

Every terminal local HTTP reply must use a status in `200..=599`. Resource
status fields use `edgion_resources::http_status::is_valid_final_http_status`
for validation and expose the same bounds in generated schemas. This applies
to:

- `FaultInjection.abort.statusCode`
- `Mock.statusCode`
- `RequestAccessPolicy.status`
- `RateLimitLocal.rejectStatus`
- `RateLimitRedis.rejectStatus`
- `IpRestriction.status`
- `RequestRestriction.status`

Native WAF SecLang `status` actions and phase-default deny statuses use the
same bounds at compilation. `edgion-waf` is a dependency-free leaf crate and
cannot call the resources predicate; resource validation tests compare WAF
compilation against the shared predicate at the boundary. Dynamic Wasm local
responses map invalid statuses to 500; ExtProc immediate replies reject them.
Request and response consumers independently enforce the final-status range.
Failed final-writer claims cannot be reported as successful delivery; a
request-stage plugin reply never writes its body after a failed header claim.
An invalid request-plugin `ErrResponse` becomes a terminal 500.

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
the regression test compares their results for every affected plugin. Native
WAF previously accepted `100..=199` in three semantic checks, leaving an
unwritable terminal status. The WAF compiler now rejects it before publication.

## Alternatives and limits

The fix intentionally rejects `100`, `101`, and `199` during configuration
validation instead of adding special runtime handling for informational
responses. A local reply is terminal, so permitting an interim
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
types. WAF semantic and resource tests cover the same boundaries, including
phase defaults; the dynamic WAF scenario uses an invalid `status:100` update to
check last-known-good retention. The response-stage invalid-status writer
remains covered by unit tests because compiled WAF rules can no longer create
that state.

Re-evaluate this rule if the canonical final-status predicate changes, if Pingora
gains an explicit informational-response API used by request plugins, or if a
listed field stops representing a terminal local reply. Also re-evaluate if
`edgion-waf` gains a suitable shared dependency for the predicate.

## Resolution references

- `use-canonical-final-http-status-validation-for-local-replies` — fixed on
  2026-09-14; original finding in
  `tasks/todo/use-canonical-final-http-status-validation-for-local-replies.md`.
- `unify-terminal-local-reply-status-validation` — WAF, dynamic producers,
  request writer, and response writer contract completed in this repair.
