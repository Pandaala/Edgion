---
name: pingora-body-buffer-replay-cancel-safety
description: Use when reviewing the pingora fork's early request body buffering seam for cancellation safety; covers the two-phase peek/consume replay API that makes select!-cancelled next_chunk calls lossless.
status: fixed
finding_id: pingora-body-buffer-next-chunk-cancel-safety
closed: 2026-07-26
knowledge_type: regression-guards
---

# Body-Buffer Replay `next_chunk` Cancellation Safety — FIXED (two-phase peek/consume)

## Conclusion

Fixed structurally in the pingora fork (`/Users/caohao/ws2/pingora`, branch `edgion`):
`RequestBodyBuffer::next_chunk` is now a **pure peek** (no cursor movement) and a new
**synchronous** `consume(bytes)` method commits consumption. The
`RegisteredRequestBodyBuffer` wrapper applies the pending commit synchronously at the
start of the following `next_chunk()` call and records the new pending length in the
same poll that returns the chunk, so a replay future dropped mid-poll by the proxy's
`tokio::select!` body pump can never lose or duplicate a chunk.

Fix suggestions of the form "document a commit-in-same-poll contract and trust each
impl to honor it" (the doc-only Option A) are **superseded** — the structural API was
chosen precisely because that contract was easy to violate silently.

## Core Rationale

**1. The proxy pump cancels replay futures routinely**

`pingora-proxy/src/proxy_h1.rs` / `proxy_h2.rs` poll `read_body_or_idle` inside
`tokio::select!` against always-armed branches (upstream response `rx.recv()` etc.).
Any branch winning drops the replay future at whatever internal await it is suspended
on (disk I/O for a spill impl). Under the old consume-on-read API, an impl advancing
its cursor before its I/O completed silently skipped a chunk — corrupt upstream body,
no error anywhere.

**2. The wrapper could not fix the old API**

When the future was dropped, the wrapper never received the chunk, so it had nothing
to stash and re-serve. Cancellation safety had to move into the API shape: peek is
repeatable, and the commit is synchronous (zero await points), hence uncancellable.

**3. `rewind` has no cancellation exposure**

`begin_request_body_replay` runs outside any `select!` (before the pump starts) in
proxy_h1/h2/custom; noted in the trait doc. The wrapper also clears the pending
commit before rewinding, so a retry never skips the first chunk.

## Guidance for the Edgion disk-spill impl (the intended consumer)

- `next_chunk` must stay an idempotent, side-effect-free read at the logical cursor:
  positional reads (`FileExt::read_at` in `spawn_blocking`), never the fd seek
  position; never keep partial read state in `self` across await points.
- `consume` is just "advance the logical cursor by N" — keep it a plain field update.
- Copy the poll-once-then-drop test harness
  (`replay_survives_select_cancellation` in `pingora-core/.../v1/server.rs`) against
  the disk impl. Also copy the mid-Replaying retry-rewind harness (BC-011):
  `rewind_from_mid_replay_re_serves_full_body` (v1) and
  `test_early_body_buffer_rewind_from_mid_replay_h2` (v2) — multi-chunk replay,
  rewind with a pending commit, and rewind after a cancelled in-flight read; this
  is where cursor vs pending-commit vs fd state is most likely to go wrong.

## Re-evaluation Triggers

Re-open only if:

- The seam trait is restructured again (e.g. for upstreaming) and the peek/consume
  split is removed.
- A new call site polls `RegisteredRequestBodyBuffer::begin_replay`/`rewind` inside a
  `select!` (its cancellation exposure was verified absent, not designed away).

## Reference Cases

- Adversarial review round 3 (2026-07-26), issue file
  `pingora-body-buffer-next-chunk-cancel-safety.md` (closed; sibling capture-side
  issue `pingora-body-buffer-capture-cancel-poison.md` tracked separately).
- Fix + regression tests: `pingora-core/src/protocols/http/body_buffer.rs`
  (`cancelled_next_chunk_re_serves_the_same_chunk`,
  `cancellation_after_delivery_neither_duplicates_nor_skips`,
  `rewind_discards_pending_commit`) and
  `pingora-core/src/protocols/http/v1/server.rs`
  (`replay_survives_select_cancellation`).
