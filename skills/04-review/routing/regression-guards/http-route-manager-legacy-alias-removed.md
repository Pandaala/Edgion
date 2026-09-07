---
name: http-route-manager-legacy-alias-removed
description: Use when reviewing HTTP route-manager naming findings; covers the removed RouteManager/get_global_route_manager legacy aliases and the still-open gRPC counterpart.
status: fixed
finding_id: gw-http-match-03
closed: 2026-07-27
knowledge_type: regression-guards
---

# HTTP `RouteManager` / `get_global_route_manager` Legacy Aliases — REMOVED

## Conclusion

`edgion-gateway/src/routes/http/routes_mgr.rs` used to carry a self-declared
compatibility shim:

```rust
// Legacy type aliases for backward compatibility.
pub type RouteManager = GlobalHttpRouteManagers;
pub fn get_global_route_manager() -> &'static GlobalHttpRouteManagers { ... }
```

Both are deleted. The canonical names are `GlobalHttpRouteManagers` and
`get_global_http_route_managers()`, and they are now the only names — the two
re-export sites (`routes/http/mod.rs`, `routes/mod.rs`), the production caller
(`api/mod.rs::store_stats`) and the 29 `routes/http/tests.rs` fixtures were
renamed in the same change.

Re-reports of the form "`RouteManager` is a legacy alias that violates the
pre-1.0 no-compatibility policy" are **stale** for HTTP — verify the symbol
still exists before filing.

## Core Rationale

**1. Pre-1.0 policy is binding, and the comment named itself**

`AGENTS.md`/`CLAUDE.md` forbids keeping "deprecated fields, legacy aliases,
migration shims, or `#[deprecated]` re-exports 'for compatibility'". A shim
whose own comment says "for backward compatibility" is the literal target of
that rule, not a judgment call.

**2. There was no compatibility boundary to preserve**

`edgion-gateway`'s lib is not consumed by any other workspace crate
(`controller`/`cli` do not depend on it), and the real names were already used
in the same files. The alias bought nothing except a second name a reader had
to learn was synonymous.

**3. Zero-risk removal**

Deleting a `pub type` alias makes every missed reference a compile error, so
the rename cannot silently change behavior. Both names resolved to the same
`OnceLock` singleton, so `store_stats` observes the identical instance.

## Fix Suggestions Not Accepted

- "Keep the alias but mark it `#[deprecated]`" — a `#[deprecated]` re-export is
  explicitly named by the same policy line.
- "Rename `GlobalHttpRouteManagers` to `RouteManager` instead (shorter name
  wins)" — `Global*RouteManagers` is the consistent shape across all five route
  families (TCP/UDP/TLS/HTTP/gRPC); collapsing HTTP to a distinct short name
  re-introduces the asymmetry from the other direction.

## Re-evaluation Triggers

- The gRPC twin `GrpcRouteManager` (`routes/grpc/`) still carries the identical
  pattern; it was out of scope for this unit (review unit G04 / the `X12`
  `x-residue` sweep). Removing it is the same mechanical change and is still
  open.

## Reference Cases

- Origin finding: `tasks/fable-review/gw-http-match-03-...` (closed 2026-07-27).
- Route-family naming matrix (the "Legacy alias" row now shows `—` for HTTP):
  [route-architecture-consistency.md](../intentional-design/route-architecture-consistency.md).

## Resolution references

- `gw-http-match-03` — 2026-07-27.
