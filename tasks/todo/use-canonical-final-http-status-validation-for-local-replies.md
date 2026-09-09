# Use Canonical Final HTTP Status Validation for Local Replies

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | correctness-unification |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Make every request-stage local-reply plugin reject informational status codes that cannot claim the canonical final HTTP response.

## Requirements

- Reuse `edgion_resources::http_status::is_valid_final_http_status` in FaultInjection, Mock, RequestAccessPolicy, RateLimit, RateLimitRedis, IpRestriction, and RequestRestriction validation.
- Reject 100, 101, and 199 before publication; retain 200 through 599 according to the canonical predicate.
- Cover both runtime-owned `ErrResponse` plugins and plugins that write their local response before returning termination.
- Raise every affected schemars minimum from 100 to 200 so generated schema and runtime validation agree.
- Keep validation messages fixed and free of internal response-writer detail.
- Preserve shared Controller/Gateway validation parity and update all colocated resource tests.
- Align any EN/ZH field documentation that currently advertises 100 through 599.
- Do not change PluginRuntime response ownership, Pingora response writing, schema field names, or existing integration tests.

## Acceptance criteria

- All seven request-stage local-reply status fields use the same final-response validity rule across both response ownership shapes.
- Informational statuses are rejected consistently during Controller and Gateway plugin validation.
- Valid 2xx through 5xx configurations retain current status/body/header behavior.
- Resource and plugin-compilation tests, workspace checks, and repository guards pass.
