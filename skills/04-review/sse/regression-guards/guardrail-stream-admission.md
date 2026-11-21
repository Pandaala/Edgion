---
name: guardrail-stream-admission
description: Use when reviewing Guardrail admission for Chat Completions, provider errors, and first-window holds.
knowledge_type: regression-guards
---

# Guardrail request admission and provider errors

## Conclusion and scope

When both AiProxy and the response-side Guardrail match a Chat Completions
request, Guardrail requires `stream: true`. Omitted or false values select
unary Chat and produce a local `400 unsupported_parameter` before provider
dispatch. Both plugin orders and both `failOpen` settings follow this rule.
The request-only AiGuard has its own admission contract.

## Rationale and evidence

In `gw-ai3-01`, unary Chat could reach the provider. Its successful JSON
response and provider errors both reached Guardrail's precommit failure
branch. Fail-open released the unreviewed completion, and fail-close replaced
the provider's response with a 503.

In `ai/http/proxy.rs`, AiProxy has the canonical stream choice and can check
the earlier semantic guard claim. In the opposite order,
`Guardrail::run_request` can read `requested_streaming` from the ready attempt
plan. When the recorded upstream status and the working response status
are both 4xx or 5xx, `Guardrail::run_upstream_response` uses
`disable_semantic_guard` to release the Guardrail claim, including its optional
first-window hold. Provider error conversion and the cooldown owner's
normalization of `Retry-After` still follow the route's existing rules.
A response filter can change the working status while retaining a successful
provider's model output, so the working status alone cannot authorize a skip.

## Verification and re-evaluation

Check omitted and explicit false stream values in both plugin orders. On an
admitted stream, check final provider errors with each failure policy and
with the first-window hold enabled. Confirm the response companion leaves the
provider error available to the route and releases its hold before commit.
Successful SSE responses retain the existing
[content-encoding policy](../../http-body/regression-guards/identity-content-encoding-classification.md).
Revisit these boundaries if Guardrail gains unary output evaluation or the AI
request-plan or final-head lifecycle changes.
