---
name: pingora-body-buffer-capture-cancel-poison
description: Use when reviewing the pingora fork's early request body buffering seam for capture-side cancellation safety; covers the session poison flag that makes a cancelled capture fail closed instead of replaying a truncated body.
status: fixed
finding_id: pingora-body-buffer-capture-cancel-poison
closed: 2026-07-26
---

# Body-Buffer Capture Cancellation — FIXED (session poison flag, fail closed)

## Conclusion

Fixed in the pingora fork (`/Users/caohao/ws2/pingora`, branch `edgion`): both H1 and
H2 server sessions carry an `early_body_capture_poisoned` flag that is set immediately
before every `capture(...)` / `finish_capture()` await in `read_body_bytes` and
cleared only after the await returns `Ok`. A body-read future dropped mid-await
(select!/timeout cancellation — Edgion's body gate does exactly this) leaves the flag
set; from then on `read_body_bytes` AND `begin_request_body_replay` hard-error
(`InternalError`), so a truncated capture can never replay against the original
`Content-Length`.

Fix suggestions of the form "make capture atomic / buffer the chunk before awaiting"
are **not accepted** — the bytes are already consumed from the transport before the
capture await, so the only sound post-cancellation semantics is "this request's
buffered body is unusable; fail closed".

## Core Rationale

**1. The seam inserted awaits after the point of no return**

Stock `read_body_bytes` was cancel-safe (only await = socket read; a dropped future
consumed nothing). The seam awaits the app buffer AFTER the chunk left the transport
and was counted in `body_bytes_read` / teed to `retry_buffer`. Cancellation there
loses the chunk from both the app's view and the buffer, and (pre-fix) nothing
noticed: `finish_capture` and `begin_request_body_replay` succeeded and replayed a
truncated (possibly empty) body. Confirmed repro in adversarial review round 3.

**2. Poison, don't retry — and gate replay too**

The flag is checked at `read_body_bytes` entry (both protocols) and at
`begin_request_body_replay` entry (before the `early_body_buffer` presence check, so
a drain that dropped the buffer still fails). All flag writes are inside the existing
`if let Some(buffer)` scopes: the unregistered path never touches it and stays
byte-identical.

**3. H2 flow-control release moved before the capture await**

`release_capacity(data.len())` previously ran after the capture await, so a cancelled
capture also leaked that chunk's flow-control window. It now runs before the await
(the bytes are consumed either way); documented inline.

**4. Trait contract documents cancellation**

`RequestBodyBuffer::write`/`finish` docs now state the futures may be dropped before
completion, that the session poisons itself (impls never observe another call after a
cancelled `write`), and that `finish` must be idempotent as a defensive contract
(`InMemoryRequestBodyBuffer` is, via its `body.is_none()` guard).

## Re-evaluation Triggers

Re-open only if:

- A new call site awaits `capture`/`finish_capture` outside `read_body_bytes` without
  the set-before-await / clear-after-Ok pattern.
- The poison checks move inside the `if let Some(buffer)` scopes in a refactor
  (a drained-then-poisoned session would silently pass `begin_request_body_replay`).
- The seam is restructured for upstreaming and the flag is replaced.

## Reference Cases

- Adversarial review round 3 (2026-07-26), issue file
  `pingora-body-buffer-capture-cancel-poison.md` (closed; replay-side sibling
  [[pingora-body-buffer-replay-cancel-safety]] closed the same day).
- Fix: `pingora-core/src/protocols/http/v1/server.rs`,
  `pingora-core/src/protocols/http/v2/server.rs`,
  `pingora-core/src/protocols/http/body_buffer.rs` (trait docs).
- Regression tests: `cancelled_capture_poisons_read_and_replay` (v1, poll-once-then-
  drop via `tokio_test`), `test_cancelled_capture_poisons_h2_session` (v2, duplex +
  h2 client, manual poll until the buffer's `write()` is entered, then drop).
