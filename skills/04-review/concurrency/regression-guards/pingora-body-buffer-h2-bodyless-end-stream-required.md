---
name: pingora-body-buffer-h2-bodyless-end-stream-required
description: Use when reviewing the pingora fork's bodyless request replay seam on H2; covers why registration requires the downstream RecvStream to have ended, not just is_body_empty().
status: fixed
finding_id: pingora-h2-bodyless-01
closed: 2026-07-28
knowledge_type: regression-guards
---

# H2 Bodyless Replay Registration Requires an Ended Downstream Stream — FIXED

## Conclusion

Fixed in the pingora fork (`/Users/caohao/ws2/pingora`, branch `edgion`):
`v2::HttpSession::set_bodyless_request_replay_buffer` now requires
`request_body_reader.is_end_stream()` in addition to the pre-existing
`is_body_empty()` check. A request that declares `Content-Length: 0` but whose
HEADERS did not carry END_STREAM (downstream stream still half-open) is
rejected at registration, fail-closed.

Re-reports of the form "H2 bodyless replay can be registered while the
downstream stream is still open" are **fixed**; reject duplicates.

## Core Rationale

**1. `is_body_empty()` is a body-content predicate, not a stream-state predicate**

H2's `is_body_empty()` returns true for `body_read == 0 && (is_end_stream() ||
Content-Length: 0)`. The `Content-Length: 0` arm makes it true while the
downstream `RecvStream` is still open — a legal H2 framing (HEADERS without
END_STREAM, empty body delivered later). The bodyless replay API's contract is
"the downstream stream already ended with no body", which only
`is_end_stream()` can attest.

**2. Replay permanently shadows the live stream**

Once registered, `read_body_or_idle()` serves exclusively from the replay
buffer. Anything the still-open downstream stream sends later — the empty
DATA + END_STREAM that legally closes it, trailers, or DATA violating
`Content-Length: 0` — is never observed, bypassing normal consumption and
protocol-error handling. Dropping the open `RecvStream` at request end can
also send RST_STREAM(CANCEL) to a client that saw its request complete
normally.

**3. HTTP/1 needs no counterpart**

Under HTTP/1 `Content-Length: 0` means the body is complete by framing — there
is no per-request stream that can remain half-open — so the v1 seam's existing
`is_body_empty()` check is sufficient and was left unchanged.

**4. Edgion degrades fail-closed**

The only external caller (`session_adapter.rs::ensure_bodyless_request_replay_owner`)
maps the registration error to `BodyReadError::Incomplete`, so a bodyless body
mutation on a CL:0-but-open request is refused rather than silently shadowing
the stream.

## Fix Suggestions Not Accepted

- Accept CL:0-but-open registrations and drain/monitor/reset the live stream —
  adds a downstream-handling design for a corner case with no product need;
  the fail-closed rejection preserves the documented contract instead.
- Loosen the ordinary capture API in compensation — unchanged on purpose; it
  still supports HEADERS-without-END_STREAM followed by an empty END_STREAM
  DATA frame (see `pingora-body-buffer-h2-end-stream-replay-source.md`).
- Drop the `is_body_empty()` check and rely on `is_end_stream()` alone — would
  admit requests whose non-empty body was already fully read, which the
  bodyless seam must not accept.

## Re-evaluation Triggers

Re-open this decision only if:

- A product path genuinely needs body injection for `Content-Length: 0`
  requests whose stream is still open (then the drain/monitor/reset design
  must be specified, including trailers and violating-DATA behavior).
- The seam gains explicit downstream-stream ownership transfer at
  registration time.

## Reference Cases

- Origin finding: `tasks/h2-bodyless-replay-requires-ended-downstream-stream.md`
  in the pingora fork, closed 2026-07-28.
- Regression test: `bodyless_replay_buffer_rejected_while_downstream_stream_open_h2`
  (v2 session; covers both CL:0-without-END_STREAM and no-CL-without-END_STREAM).
- Success-path test (unchanged): `finalized_replay_buffer_injects_body_into_empty_h2_request`.
- Sibling seam entries: `pingora-body-buffer-h2-end-stream-replay-source.md`,
  `../architecture/bodyless-request-replay-owner.md`.

## Resolution references

- `pingora-h2-bodyless-01` — 2026-07-28.
