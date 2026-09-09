# Validate Rate-Limit Response Header Names Canonically

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | correctness-security |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Validate the shared RateLimit and RateLimitRedis response-header configuration once at the resources boundary so invalid, duplicate, framing, and hop-by-hop names fail closed with Controller/Gateway preparse parity.

## Requirements

- Inventory every `LimitHeaderNames` field and both plugin consumers before changing the contract.
- Reuse the canonical HTTP header-token and configured-header size rules; do not introduce a plugin-local parser.
- Reject invalid or oversized names, case-insensitive duplicates, and canonical framing or hop-by-hop names including `content-length` and `transfer-encoding`.
- Preserve omission semantics for optional headers, default `X-RateLimit-*` names, response ordering, local-body framing, gRPC behavior, and RateLimit/RateLimitRedis failure policies.
- Update English and Chinese RateLimit and RateLimitRedis documentation atomically with the accepted schema.
- Keep existing integration tests unchanged unless implementation proves an observable gap that cannot be covered by deterministic resource and plugin tests; if so, create a separate integration issue instead of editing them in this task.

## Acceptance criteria

- Resources validation rejects every forbidden category deterministically and reports stable, non-sensitive errors.
- Controller and Gateway preparse produce matching rejection outcomes for both plugins.
- Colocated tests cover valid custom names, empty omission, invalid tokens, size bounds, case-insensitive duplicates, framing names, and hop-by-hop names.
- Workspace format/check/Clippy and repository guards pass.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-01 | Schedule a separate repair | The shared schema and two plugin consumers require one canonical admission change; a RateLimit-only runtime filter would leave invalid configuration accepted and create drift. |
