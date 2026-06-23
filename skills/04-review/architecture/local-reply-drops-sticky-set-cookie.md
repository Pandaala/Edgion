---
name: local-reply-drops-sticky-set-cookie
description: Use when reviewing findings of the form "sticky-session Set-Cookie is missing on a replaced response / build_response_local_reply discards the upstream header / session-persistence cookie lost when a response-phase plugin returns ErrResponse / sticky cookie not carried over onto a local or error reply". Records the accepted-as-designed decision for filter-04c #2 (2026-06-09).
---

# Local Reply Drops the Sticky-Session Set-Cookie (Accepted as Designed)

**Status**: accepted-as-designed (filter-04c #2, 2026-06-09)
**Area**: gateway / session-persistence / response local-reply

## Conclusion

When a response-phase plugin fully replaces a response by returning `ErrResponse`,
`build_response_local_reply` constructs a fresh `ResponseHeader` and discards the upstream header.
Any `Set-Cookie` (or session header) that session persistence injected by appending directly onto
the upstream header in `pg_upstream_response_filter.rs` is therefore lost for that replaced
response.

This is **accepted as designed**. It matches Envoy's behavior — a local reply is a fresh synthetic
response, and carrying the sticky-session cookie onto it would require extra machinery for a rare
combination (sticky route + a response-phase plugin replacing that exact response) with no
consensus benefit. Future reviewers should not re-file this as a bug.

## Core Rationale

**1. The injection site is intentionally outside `pending_response_header_ops`**

Session persistence injects its `Set-Cookie` (or session header) in
`pg_upstream_response_filter.rs` (`SessionPersistenceType::Cookie` arm) by calling
`upstream_response.append_header("Set-Cookie", ...)` directly on the upstream
`ResponseHeader` — NOT by queuing a `PendingHeaderOp`. The queued-op mechanism
(`apply_pending_response_header_ops` + the builder's drain in `build_response_local_reply`) was
introduced by filter-04c to carry request-stage-queued ops onto a fresh local reply. The
session-persistence injection was explicitly left outside that queue.

**2. Matches Envoy**

Envoy's stateful-session filter attaches the sticky `Set-Cookie` on the encode path, keyed off a
selected upstream host. On a no-host or replaced reply (error, local response), Envoy does not
emit the sticky cookie — there is no guarantee in the Envoy stateful-session semantics that the
cookie survives a replaced or error response. Edgion's behavior is consistent with this precedent.

**3. The combination is rare and the tradeoff is clear**

Carrying the sticky `Set-Cookie` onto a local reply would require either:
- Moving the injection into `pending_response_header_ops` (changing the session-persistence
  architecture to make it order-dependent on the pending-ops queue drain), or
- Explicitly re-applying session-persistence state inside `build_response_local_reply` (coupling
  the local-reply builder to session-persistence logic).

Both options add machinery and cross-module coupling for a rare combination: a route must have
session persistence enabled AND a response-phase plugin on that same route must replace that exact
response. No production use case has been identified that requires continuity here.

## Why This Is Not a Bug to Re-file

The queued-op carry-over fixed in filter-04c (`apply_pending_response_header_ops` + the builder's
drain) is deliberately scoped to `pending_response_header_ops` — ops that request-stage plugins
enqueued and that must survive a response replacement to take effect. The session-persistence
`Set-Cookie` is intentionally outside that queue.

If a future reviewer flags "sticky cookie missing on a replaced response," that is expected
behavior, not a regression. The code comment at the injection site (see
`pg_upstream_response_filter.rs`, `SessionPersistenceType::Cookie` arm) documents this explicitly.

## Fix Suggestions Not Accepted

- Move session-persistence injection into `pending_response_header_ops` — changes the
  session-persistence architecture and couples it to the pending-ops drain order; cross-module
  impact for an unconfirmed use case.
- Re-apply the session cookie inside `build_response_local_reply` — couples the local-reply
  builder to session-persistence state; adds a second injection point that could diverge from
  the normal path.
- Carry the upstream `Set-Cookie` verbatim onto the local reply — would replay a backend-selected
  cookie onto a synthetic response that may come from a different logical source (the plugin),
  potentially misleading the client.

## Re-evaluation Triggers

Re-open this decision only if:

- A concrete production use case is identified where sticky-session continuity on a replaced
  response is required and the complexity cost is justified.
- The session-persistence injection is refactored to use `pending_response_header_ops` for other
  reasons (at which point the carry-over becomes automatic and free).
- Envoy changes its stateful-session semantics to guarantee sticky-cookie carry-over on local
  replies (aligning this decision with the updated reference).

## Reference Cases

- Finding `filter-04c #2` (`tasks/todo/filter-04c-local-reply-header-carryover.md`), 2026-06-09
  — accepted as designed; closed without code change.
- Injection site: `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_filter.rs`,
  `SessionPersistenceType::Cookie` arm (`append_header("Set-Cookie", ...)`).
- Local-reply builder: `build_response_local_reply` (drains `pending_response_header_ops` onto
  a fresh header; does not touch the upstream header).
- Sibling filter-04c fix (queued-op carry-over): `apply_pending_response_header_ops` +
  `build_response_local_reply` drain — scoped to `pending_response_header_ops` only.
