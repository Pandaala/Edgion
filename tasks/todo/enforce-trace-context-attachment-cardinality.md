# Enforce TraceContext Attachment Cardinality

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-02 |
| Status | pending |
| Type | architecture-correctness |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Define and enforce one effective TraceContext owner across global, Gateway, route, and direct filter attachments.

## Requirements

- Inventory every attachment source and effective-chain composition path.
- Define precedence or rejection semantics when more than one TraceContext filter applies to a request.
- Enforce the decision before the request hot path with Controller/Gateway parity.
- Preserve W3C propagation, access-log correlation, echo, sampling, and no-data-plane-tracing boundaries.
- Keep existing integration tests frozen; create a separate integration issue only for a proven composition gap.

## Acceptance criteria

- One matrix covers every attachment combination and outcome.
- The effective request chain contains at most one TraceContext owner.
- Deterministic composition tests cover global/Gateway/route/direct combinations and hot reload.
- Workspace format/check/Clippy and repository guards pass.
