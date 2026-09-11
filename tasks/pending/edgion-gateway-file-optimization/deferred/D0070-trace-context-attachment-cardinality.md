# D0070 — Enforce TraceContext Attachment Cardinality

## Meta
- Status: fixed (2026-09-11)
- Severity: medium
- Discovered in: B0105, TraceContext analysis
- Category: composition-semantics
- Canonical knowledge: [RequestTraceId lifecycle](../../../../skills/04-review/observability/intentional-design/request-trace-id-lifecycle.md)

## Historical evidence
TraceContext cardinality validation limits instances only within one EdgionPlugins resource. Global, Gateway, and route attachments can still compose multiple TraceContext filters for one request, causing repeated trust/reset/generate/echo decisions and ambiguous final ownership.

## Original deferral rationale
One-owner enforcement belongs to the attachment/composition layer and needs precedence and error semantics across global, Gateway, route, and direct HTTPRoute filters. A plugin-local guard cannot observe the complete effective chain.

## Original integration-test assessment
The future design should first define composition precedence and deterministic chain tests. Create a separate integration issue only if the approved effective-chain behavior cannot be verified without modifying frozen scenarios.

## Resolution

The user-approved replacement removes TraceContext and its trust/generation/sampling options. Core W3C context has one request owner; RequestId rules compose incrementally. Three independent reviews, focused unit/CRD checks, static RequestId integration and RequestIdDynamic passed. The linked knowledge page records the exact contract, verification limits and provenance.
