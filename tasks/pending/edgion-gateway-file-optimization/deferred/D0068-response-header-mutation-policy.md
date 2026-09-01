# D0068 — Unify Response-Header Mutation Policy

## Meta
- Status: needs-discussion
- Severity: medium
- Discovered in: B0104, ResponseRewrite analysis
- Category: cross-cutting-policy
- Canonical task: [`tasks/todo/unify-response-header-mutation-policy.md`](../../../todo/unify-response-header-mutation-policy.md)

## Evidence
Response headers can be mutated by ResponseRewrite, Gateway API ResponseHeaderModifier, DSL, and session-level helpers. These paths do not share one documented policy for hop-by-hop fields, framing ownership, validation errors, and operation ordering. A plugin-local expansion beyond the immediate `Content-Length` and `Transfer-Encoding` framing invariant would create another policy rather than unify them.

## Why deferred
The complete rule must inventory every mutation producer and final response boundary, distinguish removal from insertion/replacement, define protocol-specific behavior, and preserve streaming framing. It requires a shared design and coordinated validation/runtime adoption rather than an opportunistic deny-list in one plugin.

## Integration-test impact
No integration issue is created yet. The future design should first use deterministic validator and response-header tests; create a separate integration issue only if an approved cross-protocol contract cannot be proven locally.
