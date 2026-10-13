---
name: access-log-fallback-backfill-tradeoffs
description: Use when reviewing access-log degradation findings — the `LogConfig.fallback` first-healthy sink and the `POST /api/v1/access-log/backfill` endpoint (backfill.rs). Covers four accepted design boundaries a reviewer is likely to re-flag: health-based (not backpressure-based) fallback routing, the dedicated-directory file matcher, at-least-once duplication on retry, and queue-entry (not ES-ack) completion marking.
status: accepted-tradeoff
finding_id: link-sys-access-log-backfill
closed: 2026-06-17
knowledge_type: intentional-design
---

# Access-Log Fallback Sink + Manual Backfill — DESIGN-TRADEOFF (accepted)

## Conclusion

The access-log `fallback` sink (`AccessLogger` first-healthy routing) and the operator-triggered
`POST /api/v1/access-log/backfill` (`edgion-gateway/src/observe/access_log/backfill.rs`) ship with
four deliberate boundaries. Fix suggestions that try to "close" any of them are **not accepted**
as review-issue fixes — each is an intentional scope decision documented in
`docs/en/ops-guide/observability/access-log.md`.

## Core Rationale

**1. Fallback routing is health-based, not backpressure-based (Tier-1).**

When the ES primary is *healthy* but its bulk channel is momentarily full, `try_send_bulk` errors
and `AccessLogger::send` counts an `edgion_access_log_dropped_total` drop and returns — it does
**not** fall through to the fallback sink. Fallback engages only when `healthy()` is false. This
keeps "one log → at most one sink" and avoids a second routing logic (Tier-2 send-fallback) racing
the health gate. Transient saturation is backpressure, not a fault.

**2. `discover_fallback_files` matches any `{stem}.*.{ext}` in the fallback directory.**

The matcher mirrors `local_file::rotation::append_suffix` (`{stem}.{suffix}.{ext}`, e.g.
`es_fallback.1.log`, `es_fallback.2026-01-02.log`) and requires a `.` separator after the stem, so
`es_fallbackX.log` is rejected. An unrelated file that happens to share the stem *and* extension
could be picked up — the fallback path is documented as **dedicated to this sink**, so this is
acceptable and mirrors the rotation module's own matching.

**3. Backfill is at-least-once; `lines_failed` undercounts after a `break`.**

On a `send_bulk` error the run records one `lines_failed`, breaks, and leaves the file
**un-renamed** (every early-return branch preserves the file). The remaining lines stay in the kept
file and replay on the next trigger. Worst case is duplication in ES (no stable doc id), never data
loss. `lines_failed` is therefore a "did the channel close" signal, not a per-line ES-rejection
count (ES-side bulk rejections happen asynchronously in the background bulk loop and are not
counted here).

**4. `mark_backfilled` marks at queue-entry, not ES-ack.**

A file is renamed `<name>.backfilled-<ts>` once all its lines are *queued* into the ES bulk channel
(awaiting `send_bulk` = natural backpressure). A crash during the bulk loop's background retry could
still lose already-queued lines — covered by the documented at-least-once contract. Marker detection
is a `.backfilled-` substring check; it only collides if an operator deliberately embeds that string
in the fallback path.

## Fix Suggestions Not Accepted

- "Spill to the fallback sink when the ES channel is full while ES is healthy" — that is Tier-2
  send-fallback, explicitly out of scope; it duplicates the health gate and risks double logic.
- "Tighten `discover_fallback_files` to an exact rotation-suffix grammar" — the dedicated-directory
  assumption makes the extra strictness churn with no real-world benefit.
- "Make backfill exactly-once with stable `_id`s / two-phase ack" — large feature; access logs
  tolerate duplicates. Belongs in `tasks/todo/` via brainstorming, not a review fix.
- "Count every unprocessed line as failed" — the lines are retained and retried, not lost; counting
  them as failed would be misleading.

## Re-evaluation Triggers

Re-open only if:

- A use case requires access logs to be exactly-once / de-duplicated in ES (then a stable doc id +
  ack-based marking is warranted).
- The fallback directory stops being dedicated (shared with other writers), making the `{stem}.*`
  matcher unsafe.
- A Tier-2 "spill on transient channel-full" requirement is formally adopted.

## Additional items considered in the deep review (2026-06-17)

Two findings were **fixed**, not accepted — do not expect the old behavior:

- **Active-file rename data-loss (was Critical):** backfill renames the LocalFile sink's *active* file; `reopen_sink_if_renamed` in `local_file/data_sender_impl.rs` reclaims the base during before-batch and idle recovery. The original presence-only check missed a healthy rename followed by recreation before the checkpoint (ARCH-026). Unix recovery now compares descriptor-derived device/inode identity with active-path metadata, flushes retained bytes to the old writer, and validates the candidate path before installing the replacement with its identity and current length. Metadata/open failures preserve the old writer and keep attachment unhealthy for later retry; internal rotation refuses an unconfirmed attachment. Non-Unix recovery remains presence-only and cannot guarantee detection of same-path replacement. A stable path converges at later checkpoints; concurrent external changes are not atomic with writer validation. The worker owns native Rust size/time rotation without an external utility dependency. This corrects the historical blanket claim of rename-plus-create coverage; it does not add backfill delivery acknowledgements or exactly-once replay.
- **`RotationConfig.max_files` serde default (was Important):** a partially-specified `rotation` block used to get bare-`usize` `0` (unlimited). Now `#[serde(default = "default_max_files")]` = 10, matching the struct `Default`.

The following were **considered and accepted** (do not re-flag):

- **Backfilled docs land in the current-date index, not the entry's date.** Re-ingest uses the live bulk path. Docs keep their own `@timestamp` and are findable via the `<prefix>-*` pattern. Now documented in the ops guide; not a code defect.
- **Size-rotation replay order is lexicographic** (`.10.log` sorts before `.2.log`). No correctness impact — every doc carries `@timestamp` and ES orders at query time.
- **Corrupt / torn / invalid-UTF-8 fallback lines** during backfill are best-effort: an unparsable UTF-8 line is skipped uncounted; a syntactically-broken JSON line is ES-rejected asynchronously and counted as ingested. This is the same boundary as the at-least-once `lines_failed` semantics above.
- **`es_key` (namespace/name) and the fallback path appear in backfill response bodies / `stopped_reason`.** Both are operator-configured, the endpoint is admin-auth-gated, and the values aid debugging — not a sensitive-info leak.
- **`run_backfill` uses blocking `std::fs` in an async fn.** Accepted for the small, bounded fallback files this targets; revisit only with a benchmark showing a real stall.

## Reference Cases

- Final feature review, 2026-06-17 (access-log fallback + manual backfill).
- `edgion-gateway/src/observe/access_log/backfill.rs`, `edgion-gateway/src/observe/access_log/logger.rs`
- `docs/en/ops-guide/observability/access-log.md` (degradation + backfill section)
- Related: [healthy-semantics-intentional-non-flips.md](healthy-semantics-intentional-non-flips.md)
