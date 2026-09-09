# Route Architecture Manager-Cache Documentation

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-09 |
| Status | pending |
| Type | docs |
| Priority | P3 |
| Issue | N/A |

## AI guide summary

Align route architecture knowledge with the current request path, where HTTP and gRPC proxies retain listener-keyed route-manager handles instead of dynamically resolving a manager on every request.

## Requirements

- Revalidate HTTP, gRPC, TCP, UDP, and TLS manager ownership against current code.
- Correct `route-architecture-consistency.md` statements that still describe dynamic HTTP/gRPC manager lookup.
- Preserve historical rationale while clearly distinguishing the current cached-handle design from legacy behavior.
- Cross-check retained-manager removal and recreation semantics documented by the route-manager cleanup campaign.
- Revalidate the same page's claim that only HTTP/gRPC support Gateway-driven rebuilds and its raw-route cache table. B0198 found all five route families in `runtime/handler.rs` rebuild dispatch and compiled TCP route units; do not use those stale claims as current ownership evidence.
- Recheck the shared-L4 paragraph in `skills/01-architecture/02-gateway/03-routes/SKILL.md`, which also describes raw route Arcs. The TCP-specific page now documents compiled units; keep the index and owning review page consistent without changing runtime ownership.
- Revalidate `edgion-gateway/src/routes/tls/routes_mgr.rs` module comments describing raw-schema caching and full rebuild on every change; B0203 verified compiled units and affected-listener incremental publication. Limit any correction to accurate ownership documentation.

## Scope

### In scope

- Routing review knowledge about manager lookup, cache ownership, and request-time access.
- Links to the current HTTP/gRPC proxy fields and manager invalidation behavior.

### Out of scope

- Runtime changes to manager publication, lookup, or invalidation.
- B0176's per-listener proxy-state cleanup.
- Changes to existing `edgion-tests/**` files.

## Affected modules

| Module | Impact |
|--------|--------|
| `skills/04-review/routing/intentional-design/route-architecture-consistency.md` | Replace obsolete dynamic-resolution claims with current cached-handle ownership. |
| HTTP/gRPC route managers | Source evidence only; no runtime change. |

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-09 | Defer from B0176 | The stale statement is independent documentation debt and should not expand a Pingora lifecycle cleanup. |
| 2026-09-09 | Attach B0198 findings to this existing task | Gateway-driven rebuild coverage and the raw-route cache table are related ownership-documentation drift, not TCP handler runtime defects. |
