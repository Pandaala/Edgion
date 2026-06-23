---
name: local-file-write-error-surfaced
description: LocalFileWriter's background thread now surfaces writeln! errors (debounced tracing::error! + per-log-type *_log_dropped metric) and only advances current_size on a successful write, so a disk-full / permission failure is observable and cannot drift the tracked size into premature size rotation. Reject re-reports claiming local_file write errors are silently discarded or that current_size is incremented unconditionally.
---

# link-sys F-020 (local_file) — background write errors are surfaced, `current_size` no longer drifts

**Status:** ✅ fixed · 2026-05-25
**Severity:** Low (BadLogic) — silent log loss on write failure + `current_size` over-count on error

## Conclusion

`LocalFileWriter`'s background write thread (`edgion-gateway/src/link_sys/providers/local_file/data_sender_impl.rs`)
now routes both the first-line write and every batched write through the module-level
`write_log_line(file, line, current_size, consecutive_errors, log_type)` helper. The helper:

1. Adds `line.len() + 1` to `current_size` **only when `writeln!` returns `Ok`** — a failed write
   leaves the tracked size unchanged, so it cannot exceed the real on-disk size and trigger a
   premature size-based rotation.
2. On a write error, increments the per-log-type dropped counter
   (`global_metrics().{access,ssl,tcp,tls,udp}_log_dropped()`) so the failure is visible in
   Prometheus on the same `edgion_*_log_dropped_total` metric used for queue-full drops.
3. Logs `tracing::error!` only on the **first** error of a consecutive-failure burst (debounced via
   a `consecutive_write_errors` counter) and emits one `tracing::info!(recovered_after = N)` when a
   write next succeeds, so a persistent disk-full / permission error does not flood the log.

The thread captures `let log_type = self.log_type;` before the spawn so the metric can be keyed by
log type. The `tracing::*` calls are legitimate here: this is a dedicated `std::thread`, **not** the
data-plane hot path (the hot path is `DataSender::send`, which only does a non-blocking
`try_send`); the same thread already uses `tracing::error!` for file-open / rotation / flush
failures.

Re-reports of the form "`let _ = writeln!(...)` silently discards the io::Result / write failures
produce no error, no metric, no operator signal / `current_size` is incremented even when the write
fails / healthy flag never reflects a degraded writer" are **not accepted** for the write-error and
size-drift parts — verify the current `write_log_line` helper before re-reporting.

## Fix Suggestions Not Accepted

- "Set `self.healthy = false` from the background thread on write failure (via a shared
  `Arc<AtomicBool>`) so `healthy()` reflects a degraded writer." **Not done — out of scope.** The
  issue itself marked this "Optionally". `healthy()` feeds sender failover in
  `observe/access_log/logger.rs` (`send` picks the first healthy sender); flipping it false on a
  transient disk-full would, in the common single-sender setup, route all logs nowhere and require
  recovery-flip logic that can oscillate. The transient write-failure signal belongs on the
  counter + `tracing::error!`, not on a startup-style health bool. (Contrast
  `system-error-state-metric.md`: the `edgion_system_error_state` gauge is for *startup
  configuration* degradations, not transient runtime faults, so it does not apply here either.) If
  failover-on-degraded-writer is ever wanted, track it as a separate task, not under this finding.
- "Add a new dedicated write-failure metric instead of reusing `*_log_dropped`." Not accepted —
  reusing the existing per-log-type dropped counter is what the issue's suggested fix shows; both
  queue-full and write-failure mean "a log line was lost", so one counter answers the operator's
  question. Adding a parallel metric only doubles the alerting surface.

## Re-evaluation Triggers

Re-open only if:

- The write sites revert to `let _ = writeln!(...)` or `current_size` is again advanced before the
  write result is known.
- A future requirement genuinely needs writer-level failover (then revisit the `healthy()`
  `AtomicBool` decision as its own task).

## Reference Cases

- link-sys-F020 local_file (review finding, `tasks/link/`, 2026-05-25)
- `observability/system-error-state-metric.md` — why the startup-degradation gauge does not cover
  transient write faults.
- `observability/observability-review-rule.md` — the "zero tracing on the data plane" rule and why
  the background-thread `tracing::error!` here is exempt (not the request/connection path).
