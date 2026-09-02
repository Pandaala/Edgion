# D0072 — Bound Outbound HTTP/2 Response Header Lists

## Meta
- Status: needs-investigation
- Severity: medium
- Discovered in: B0107, Wasm callout analysis
- Category: resource-bounds
- Canonical task: [`tasks/todo/bound-outbound-http2-response-header-lists.md`](../../../todo/bound-outbound-http2-response-header-lists.md)

## Evidence
Wasm callout response bodies are capped while streaming, but the shared reqwest/Hyper HTTP/2 client can accept a much larger decoded response header list before a callout-local consumer sees it. A post-parse plugin cap would be too late to prevent the allocation and would duplicate canonical outbound-client policy.

## Why deferred
The correct bound belongs in `build_outbound_client` and must be evaluated for every shared reqwest consumer, protocol defaults, interoperability, and error mapping. That cross-cutting client-policy decision is wider than B0107's file-local ownership transfer.

## Integration-test impact
The eventual shared-client policy needs deterministic oversized-header coverage. Do not modify existing integration-test files; create a separate integration issue only if a new end-to-end scenario is still needed after shared-client tests.
