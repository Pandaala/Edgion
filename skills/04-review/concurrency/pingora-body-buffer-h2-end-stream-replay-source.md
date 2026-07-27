---
name: pingora-body-buffer-h2-end-stream-replay-source
description: Use when reviewing the pingora fork's early request body buffering seam for upstream framing; covers why is_body_empty() reports non-empty while a replay buffer is registered so H2 END_STREAM-on-HEADERS tracks the replay source.
status: fixed
finding_id: BC-002
closed: 2026-07-26
---

# H2 END_STREAM Derived From Original Payload, Not Replay Source — FIXED (registered-buffer guard in `is_body_empty`)

## Conclusion

Fixed in the pingora fork (`/Users/caohao/ws2/pingora`, branch `edgion`): while an early
request body buffer is registered, `v1::HttpSession::is_body_empty()` and
`v2::HttpSession::is_body_empty()` return `false` unconditionally, mirroring the
pre-existing `is_body_done()` replay guard. The effective request body is whatever the
buffer replays — possibly a non-empty rewrite of a zero-byte original — so every
upstream framing decision derived from `is_body_empty()` (H2 END_STREAM-on-HEADERS in
`proxy_h2.rs`, the early end-of-body pipe send in `proxy_h1.rs`, H2→H1
`Transfer-Encoding: chunked` insertion) now keeps the stream open until replay EOF.

Re-reports of the form "H2 upstream HEADERS carries END_STREAM although the replay
buffer rewrites a zero-byte payload into a non-empty body" are **fixed**; reject
duplicates.

## Core Rationale

**1. Registration's empty-body rejection cannot catch zero-payload framings**

`set_request_body_buffer` rejects requests that are *known* empty at registration time,
but a framing that merely permits a body can legally deliver zero payload bytes: H1
chunked terminated by an immediate zero chunk (body reader is in `Chunked` state at
registration, `Complete(0)` only after the drain), or H2 HEADERS without END_STREAM
followed by an empty END_STREAM DATA frame (`is_end_stream()` still false at
registration). Both register successfully, capture zero bytes, and may be rewritten to
non-empty per the trait contract.

**2. The symptom was protocol-dependent and worse than an error on H1**

H2 upstream: `send_header_eos = send_end_stream && is_body_empty()` terminated the
stream at HEADERS; the subsequent replay DATA hit h2's send-after-end-of-stream user
error and the request failed. H1 upstream (same root cause, found during the fix):
`proxy_h1.rs` sent `HttpTask::Body(None, true)` early because `is_body_empty()` was
true, and the later replay chunks were silently dropped by the H1 client write path —
the request forwarded with an empty body, no error anywhere.

**3. Guard on registration, not on replay state**

The guard tests `early_body_buffer.is_some()` rather than `is_replaying()` because one
call site (`proxy_h1.rs` H2→H1 chunked-TE insertion) runs before
`begin_request_body_replay()`. Registration is already a commitment (no un-register;
drain drops the buffer and clears the guard), so "registered → effective body unknown,
treat as non-empty" is the sound semantics for every state of the buffer.

## Fix Suggestions Not Accepted

- Fix only the `proxy_h2.rs` call site — leaves the H1 silent-body-drop variant and any
  future `is_body_empty()` caller with the same trap.
- Probe the replay source for emptiness (peek `next_chunk` before the HEADERS
  decision) — the wrapper's peek/consume cursor makes a second `next_chunk` call commit
  the first chunk, so a probe would skip data unless a new side-effect-free probe API
  is added; complexity without benefit over the guard.

## Re-evaluation Triggers

Re-open this decision only if:

- The seam gains an "un-register" operation, so a registered buffer no longer implies
  replay will happen.
- The trait contract changes so replay length is known at registration time (then the
  emptiness decision could use the actual replay length).

## Reference Cases

- Finding BC-002, review batch `tasks/review-body-cache/` in the pingora fork,
  closed 2026-07-26.
- Regression tests: `zero_payload_chunked_body_stays_non_empty_while_registered`
  (v1 session), `test_zero_payload_body_stays_non_empty_while_registered_h2`
  (v2 session, sequenced so the empty END_STREAM DATA arrives after registration).
- Sibling seam entries: `pingora-body-buffer-replay-cancel-safety.md`,
  `pingora-body-buffer-capture-cancel-poison.md`.
