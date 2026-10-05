# H2-004: Latch Terminal Trailer Failure for Direct and Poll APIs

## Disposition

Deferred; blocked by the accepted upstream h2 trailer-decoder limitation and
its eventual upstream fix.

The public h2 API currently exposes both a valid empty trailer block and a
pseudo-only trailer block whose forbidden field was discarded as
`Some(empty)`. Pingora cannot safely implement this latch by treating an empty
map as either always valid or always invalid. Keep this finding open until an
upstream h2 release rejects trailer pseudo-fields and oversized trailer blocks
before publishing a trailer event.

## Severity

P1 response-integrity risk.

## Evidence

`read_trailers()` can be the first terminal operation and currently does not
consult terminal HEADERS state. The staged poll error latch handles
`poll_trailers()` errors, but a same-burst reset can produce `Ok(None)` after a
decoder rejection; direct and poll callers can then observe a clean end.

## Required change

- Introduce one permanent response trailer/error state shared by async body,
  poll body, and direct `read_trailers()` paths.
- When terminal HEADERS was observed but h2 produces no validated trailer event,
  latch a `ReadError` before returning EOF/None.
- Do not re-poll a consumed trailer stream after any terminal decision.

## Acceptance tests

- Fresh-session direct-trailer-first tests after exact-length DATA.
- Pseudo-only and mixed pseudo trailer cases with and without reset.
- Poll path where h2 returns no trailer event after decoder rejection.
- Repeated body EOF and repeated trailer reads preserve the first terminal
  decision.

## Related files

- `pingora-core/src/protocols/http/v2/client.rs`
- `../../../../../pingora/edgion-changes/pending-issues/h2-terminal-headers-completion.md`
- `skills/04-review/h2-grpc/regression-guards/h2-trailer-decoder-upstream-limitation.md`
