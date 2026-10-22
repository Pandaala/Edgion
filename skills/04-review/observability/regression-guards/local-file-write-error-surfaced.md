---
name: local-file-write-error-surfaced
description: Use when reviewing LocalFileWriter buffered writes, failed-batch loss accounting, flush-driven health and idle recovery. Distinguish pending accounting from durable delivery and exact file size.
knowledge_type: regression-guards
---

# Local-file buffered write failures, accounting and recovery

## Conclusion and scope

`LocalFileWriter` delegates background writes to `BatchSink` in
`edgion-gateway/src/link_sys/providers/local_file/data_sender_impl.rs`. Buffering
success is not a successful flush or durable delivery. Writer failures use the
existing typed dropped-log counters and debounced background diagnostics; health
combines consecutive flush failures, the disk probe and active-path attachment.

The 2026-05-25 link-sys-F020 correction originally introduced `write_log_line`
and left writer-driven health outside that finding's scope. That helper and the
old rejection of runtime health flips are superseded. They must not guide edits
to the current BatchSink implementation.

## Rationale and evidence

- `write_line` adds `line.len() + 1` to `pending_bytes` and one to `pending_lines`
  only when `writeln!` succeeds. A buffering error records one typed drop and
  emits a debounced error. It does not advance `current_size` or directly
  increment the consecutive-flush-error count.
- `flush` success adds the current pending bytes to `current_size`, clears the
  buffering-error debounce and resets consecutive flush errors. Failure leaves
  `current_size` unchanged, increments consecutive flush errors, and adds the
  current pending line count once through `LogType::record_drops`. Both outcomes
  clear pending accounting and republish health. Zero counts do not register a
  dropped metric; repeated failures without new pending lines do not recount
  the previous batch.
- A writer is healthy only when consecutive flush errors are below three,
  `disk_ok` is true and `path_ok` is true. Disk or attachment failures can therefore
  shed it before the flush-error threshold. Queue saturation counts a drop but
  does not change health; a disconnected queue marks the writer unhealthy.
- The worker uses `recv_timeout` with an idle interval clamped to at least one
  second. Idle processing checks attachment and disk usage, then retries a flush
  when flush errors remain. A successful retry clears that error state and
  republishes combined health without requiring new traffic. This matters because
  first-healthy routing sends no new entries to an unhealthy sink. A failed
  flush need not imply a nonempty buffer: the underlying writer's flush itself
  can fail. Recovery must not depend on that assumption.

The dedicated writer thread may emit its existing bounded failure/recovery
tracing. `DataSender::send` stays nonblocking and uses metrics without request-
path tracing. The canonical
[typed-dispatch ownership](access-log-all-senders-unhealthy-metric.md) prevents
self-counting senders from also returning a dispatch error that would be counted
again. Do not add another metric family for the same loss signal.

## Limits and related decisions

The dropped count is best-effort pessimistic, not proof that every counted byte
was lost. BufWriter can retain bytes after a failed write; partial writes can
already have reached the underlying file, and later recovery can deliver retained
bytes. Failed-batch accounting has already been cleared, so later delivery does
not reconstruct the original size accounting. `current_size` is not an exact
filesystem-length oracle across failures, and successful flush is not an fsync
or remote delivery acknowledgement. This page establishes no maximum size drift,
maximum batch byte size, exactly-once delivery or hard file-size cap.

[Health tradeoffs](../../link-sys/intentional-design/healthy-semantics-intentional-non-flips.md)
own queue saturation and live idle recovery. This correction does not reopen the
accepted approximate size-accounting policy. For descriptor identity,
flush-before-replacement, candidate validation and before-batch/idle attachment
checks, use the current ARCH-026 evidence in
[fallback and backfill](../../link-sys/intentional-design/access-log-fallback-backfill-tradeoffs.md).
The proposed probe-cadence performance investigation grants no recovery-policy
change. First-healthy selection remains health-based, without send-error fallback.

## Verification and re-evaluation

At `972f0987f`, fresh
`cargo test -p edgion-gateway --lib link_sys::providers::local_file::data_sender_impl::tests::`
passed 25 cases. Relevant tests include
`failed_batches_count_pending_lines_once_for_each_log_type`,
`batch_sink_flips_unhealthy_after_threshold_flush_failures`,
`batch_sink_recovers_on_successful_flush`, `batch_sink_write_line_error_debounces`,
`batch_sink_idle_retry_recovers_after_failures_clear`, and the rename/recreation
and rotation recovery cases. They exercise injected writers and temporary files;
they are not live disk-full, durability or performance measurements.

Revalidate this guidance when pending accounting, flush behavior, health signals,
queue routing or recovery scheduling changes. Do not reject a new finding solely
because the original F020 issue was fixed. The current implementation and the
specific failure sequence must support any claimed guarantee.

## Resolution references

- link-sys-F020 local_file, original write-error correction, 2026-05-25.
- `972f0987f`: failed-batch counters aggregated while retaining existing semantics.
- Unit tests: `edgion-gateway/tests/unit/link_sys/providers/local_file/data_sender_impl_test.rs`.
- Canonical tracing boundary: `skills/03-coding/01-log-safety.md`.
