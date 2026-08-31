# Bound upstream-authority template rendering before allocation

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | skipped |
| Type | optimization |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Superseded by [`unify-gateway-request-template-runtime`](unify-gateway-request-template-runtime/unify-gateway-request-template-runtime.md), which absorbs this task's 63-byte authority budget and DNS-label acceptance into the required canonical evaluator design and also closes the destination-template variable-admission gap.

## Requirements

- Reuse the shared compiled-template engine; do not add a plugin-local parser or variable resolver.
- Add an explicit bounded-evaluation policy that preserves the authority path's current missing-variable-as-empty semantics.
- Check every append with overflow-safe arithmetic before growing the output beyond 63 bytes.
- Preserve verbatim variable handling, the fixed non-rendered suffix, lowercase DNS normalization, IP-literal rejection, and the existing `RenderEmpty` versus `InvalidLabel` outcomes.
- Avoid logging rendered labels or source values.
- Do not weaken the Service-anchor and pinned-resolution protections for outbound calls.

## Scope

### In scope

- Shared template evaluation API needed by upstream authority.
- `backends/policy/backend_traffic/authority.rs` rendering and focused tests.

### Out of scope

- General template-engine migration for unrelated plugins.
- Dynamic DNS destinations outside the fixed wildcard suffix contract.

## Acceptance criteria

- Labels of exactly 63 bytes succeed and 64-byte or larger cumulative renders fail before the output allocation grows past the limit.
- Multi-segment templates enforce the aggregate limit, not a per-segment limit.
- Missing variables preserve current empty-render behavior and error classification.
- Separator injection, leading/trailing hyphens, lowercase normalization, and composed IP-literal tests remain fail-closed.
- A focused allocation test proves oversized input does not produce an oversized temporary output String.
- Workspace check, Clippy, and repository guards pass.

## Related guidance

- `skills/00-design-rules.md`
- `skills/01-architecture/05-resources/20-edgion-backend-traffic-policy.md`
- `skills/02-features/03-resources/20-edgion-backend-traffic-policy.md`

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-31 | Defer implementation from the file audit. | Reusing the current bounded evaluator would change missing-variable semantics; the shared engine needs an explicit policy and cross-consumer tests rather than an authority-local copy. |
| 2026-08-31 | Superseded by the canonical request-template runtime task. | Running this narrow task separately would duplicate the same bounded evaluator seam; its acceptance criteria move intact to the wider task, which also enforces restricted destination variables. |
