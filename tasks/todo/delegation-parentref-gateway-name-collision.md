# Cover Delegation and Gateway ParentRef Name Collisions

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | testing |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Add live HTTPRoute and GRPCRoute regression coverage proving that a delegation-only child cannot attach directly to a same-coordinate Gateway, while the child remains reachable only through the delegating parent route.

## Requirements

- Cover HTTPRoute and GRPCRoute independently with a Gateway whose namespace/name collides with the child's delegation parentRef namespace/name.
- Prove the delegation-only child is not directly reachable through the colliding Gateway attachment.
- Prove the same child behavior remains reachable through the parent route that explicitly delegates to it.
- Exercise both the initial publication and a relevant update so stale status or route-table state cannot hide the collision.
- Assert route status and live traffic outcomes without depending only on raw configuration presence.
- Preserve delegation prefix/service-scope rules, ReferenceGrant behavior, route ordering, and existing parent/child status semantics.
- Keep traffic observability in the existing access-log path; do not add tracing, metrics, or sensitive diagnostics for the test.
- Do not modify any existing `edgion-tests/**` file without explicit path-level authorization. Implement this deferred coverage only after separate authorization releases the exact required integration paths.

## Scope

### In scope

- A live HTTPRoute delegation collision scenario.
- A live GRPCRoute delegation collision scenario.
- Status and request assertions that distinguish direct attachment from delegated reachability.

### Out of scope

- Further parentRef identity implementation changes.
- New delegation semantics, configuration fields, logging, or metrics.
- Changes to existing integration files without explicit authorization for their exact paths.

## Acceptance criteria

- In both HTTP and gRPC scenarios, direct traffic that would require treating the delegation parentRef as a Gateway attachment is rejected or unmatched.
- Traffic through the valid delegating parent reaches the child behavior successfully.
- Status assertions distinguish the Gateway and route-kind parent identities at the same namespace/name coordinates.
- The scenario remains deterministic across initial load and update without fixed sleeps.
- The separately authorized integration paths, format/check gates, and relevant documentation validation pass.
