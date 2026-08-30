# Bound upstream-authority template rendering before allocation

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | pending |
| Type | optimization |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Render the dynamic `upstreamAuthority` wildcard label under its 63-byte DNS limit instead of building the complete request-derived string and rejecting it afterward. The current `render_authority` calls the unbounded convenience template evaluator, then `validate_label`; an oversized header, query, path, URI, or context value therefore allocates and copies data that can never be a valid label. The existing bounded evaluator is not a drop-in replacement because it treats a missing variable as an error, while the authority path currently renders missing variables as empty and distinguishes only an empty final label from other invalid labels.

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

- Changing allowed template variable types or authority schema.
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
