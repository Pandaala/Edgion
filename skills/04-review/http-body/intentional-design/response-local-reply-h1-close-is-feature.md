---
name: response-local-reply-h1-close-is-feature
description: Use when reviewing findings that response-stage local replies close an HTTP/1 downstream connection instead of preserving keepalive. Records that the current close is protocol-correct and reuse is a separate performance feature.
knowledge_type: intentional-design
---

# HTTP/1 close after a response-stage local reply is a feature gap

**Status**: safe current behavior; performance feature promoted (filter-04c #3, 2026-08-03)
**Area**: gateway / HTTP/1 lifecycle / response-stage local reply

## Decision

Do not classify the current connection close as a correctness defect. The
response-stage local-reply path calls `session.set_keepalive(None)` before
writing the complete synthetic response, so Pingora's HTTP/1 codec accurately
advertises `Connection: close` and the socket is then closed. A client is not
invited to pool a connection that unexpectedly disappears.

Keeping the connection reusable is a performance feature. It requires an
explicit cross-repository design because the Edgion Pingora fork currently
honors `FailToProxy.can_reuse_downstream` only before upstream peer selection.
The response-stage sentinel error occurs after the upstream pump starts, and
that post-pump path discards the reuse verdict before finalization.

Removing only Edgion's `set_keepalive(None)` is not accepted: it would advertise
keepalive without changing the post-pump close. A safe feature must propagate a
reuse permission through Pingora while leaving codec-level framing, request-body
drain, overread, shutdown, and other transport checks authoritative.

## Review guidance

- Close reports framed as a protocol error, unexpected close, or incorrect
  local reply unless they demonstrate a distinct framing mismatch.
- Treat the extra connection setup as a P3 performance enhancement.
- Re-open a correctness issue only if the wire advertises keepalive and then
  unavoidably closes, the local reply is written more than once, or replaced
  upstream bytes reach the client.
- Do not conflate this item with sticky-session `Set-Cookie` carry-over or
  pending response-header operation carry-over.

## Feature handoff

The complete brainstorming handoff was created in the dedicated working tree at
`tasks/todo/http1-response-local-reply-keepalive/`. It covers the Pingora API
contract, lower-level safety gates, candidate designs, open questions, test
strategy, and repository sequencing.

## Resolution references

- `filter-04c #3` — 2026-08-03 (feature promoted).
