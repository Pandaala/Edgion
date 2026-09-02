# D0070 — Enforce TraceContext Attachment Cardinality

## Meta
- Status: needs-discussion
- Severity: medium
- Discovered in: B0105, TraceContext analysis
- Category: composition-semantics
- Canonical task: [`tasks/todo/enforce-trace-context-attachment-cardinality.md`](../../../todo/enforce-trace-context-attachment-cardinality.md)

## Evidence
TraceContext cardinality validation limits instances only within one EdgionPlugins resource. Global, Gateway, and route attachments can still compose multiple TraceContext filters for one request, causing repeated trust/reset/generate/echo decisions and ambiguous final ownership.

## Why deferred
One-owner enforcement belongs to the attachment/composition layer and needs precedence and error semantics across global, Gateway, route, and direct HTTPRoute filters. A plugin-local guard cannot observe the complete effective chain.

## Integration-test impact
The future design should first define composition precedence and deterministic chain tests. Create a separate integration issue only if the approved effective-chain behavior cannot be verified without modifying frozen scenarios.
