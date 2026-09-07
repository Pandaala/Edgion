---
name: request-body-over-limit-immediate-reject
description: Use when reviewing whether an over-limit downstream request body should be drained before the gateway returns 413.
status: fixed
finding_id: BF-02
closed: 2026-07-28
knowledge_type: regression-guards
---

# BF-02 — Over-Limit Request Bodies Are Rejected Without Draining

## Conclusion

Once request-body capture crosses the gateway-wide physical cap, the gateway
returns `BufferLimitExceeded` immediately. It does not drain the rest of the
client-controlled body.

HTTP/1.x connection reuse is disabled before the error is returned. HTTP/2
keeps the multiplexed connection alive: the 413 response closes the response
side of only that stream, and dropping the unfinished request side makes the
`h2` transport send `RST_STREAM(NO_ERROR)`.

## Core Rationale

### Drain work cannot remain client-controlled

A per-read timeout does not bound total drain time. A chunked HTTP/1.1 client
can send one small chunk before every read deadline indefinitely, while
Pingora's generic downstream read timeout is a no-op for HTTP/2. Once the
physical cap has already decided the response, further reads provide no
business value and must not delay 413.

### Connection handling is protocol-specific

HTTP/1.x cannot safely reuse a connection that still contains request-body
bytes, so `set_keepalive(None)` makes the response-completion path close it
without another drain. Closing an entire HTTP/2 connection would terminate
unrelated concurrent streams, so `set_keepalive(None)` is intentionally a
no-op there. Pingora writes the bodyless 413 with END_STREAM; when the session
is released, the `h2` crate applies the RFC early-response behavior and resets
the still-open request side with `NO_ERROR`.

### Partial capture is request-scoped and discarded

The registered replay buffer remains owned by the rejected request and is
dropped with its session. Returning at the first observed over-limit chunk
prevents every later client chunk from reaching memory or spill-file capture.
No incomplete body can be replayed upstream.

## Fix Suggestions Not Accepted

- Drain with only the downstream per-read timeout — a slow sender can refresh
  that timeout forever, and HTTP/2 does not apply it.
- Add a configurable drain deadline — rejected requests do not need connection
  reuse badly enough to justify another operator-facing timeout or delayed 413.
- Close the entire HTTP/2 connection — it would terminate unrelated streams
  and discard multiplexing.
- Call `shutdown()` before writing 413 — the client may observe only a reset or
  connection close instead of the intended response.

## Re-evaluation Triggers

Re-open this decision only if:

- Pingora changes HTTP/2 early-response finalization so dropping an unfinished
  request stream no longer sends the RFC-compatible reset.
- A future transport can reuse a request channel with unread bytes but does not
  provide stream-scoped cancellation on session release.

## Reference Cases

- `edgion-gateway/src/plugins/runtime/session_adapter.rs`
- `edgion-tests/src/client/suites/edgion_plugins/body_plugins.rs`
- BF-02 review batch, 2026-07-28

## Resolution references

- `BF-02` — 2026-07-28.
