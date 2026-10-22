---
name: access-log-all-senders-unhealthy-metric
description: AccessLogger records selected-sender errors and all-senders-unhealthy loss on the configured LogType counter, while disabled zero-sender loggers remain silent and LocalFileWriter self-counts internal loss through the same canonical operation.
knowledge_type: regression-guards
---

# observe F-003 — typed log-dispatch loss uses the canonical dropped counter

**Status:** ✅ fixed · 2026-05-28; typed-dispatch correction 2026-09-20
**Severity:** Medium (BadLogic) — silent or misclassified log loss

## Conclusion

`AccessLogger` stores its `LogType`. `AccessLogger::send` and
`AccessLogger::send_with_meta` call the canonical `LogType::record_drops` operation when the first
healthy remote sender returns `Err` or when registered senders exist but all report unhealthy.
Consequently Access, TCP, TLS, and UDP async loggers increment only their corresponding
`edgion_*_log_dropped_total` counter. `AccessLogger::new` and `Default` retain Access semantics;
`create_async_logger` propagates its requested type through the typed constructor.

A logger with zero senders remains a silent disabled no-op. Dispatch still selects only the first
healthy sender and does not fall through after a send error.

`LocalFileWriter` is deliberately different from remote senders: queue and background write loss
is known inside the writer, so it calls the same `LogType::record_drops` operation internally and
its dispatch method returns `Ok(())`. Remote senders return `Err`, so `AccessLogger` counts that
loss exactly once. A sender must not both self-count a dispatch failure and return `Err`, because
that would double-count one lost entry.

The shared operation accepts a line count. Single-entry losses pass one; a failed
flush adds its pending line count once, then clears pending accounting as before.
Zero counts do not initialize or register a dropped counter. This preserves the
best-effort pessimistic total: retained BufWriter bytes can still reach disk after
a later recovery, and an empty retry does not count those lines again. A scrape
may observe the aggregate update rather than intermediate per-line increments.

## Core Rationale

**1. One typed counter family answers the operator question**

The existing dropped-counter family is the canonical signal for lost Access, SSL, TCP, TLS, and
UDP log entries. Reusing it for selected-sender errors and the all-unhealthy case avoids a second
metric and prevents non-Access loggers from polluting the Access counter.

**2. Sender and dispatcher ownership are complementary**

Local-file queue saturation, disconnects, open failures, buffering failures, and flush failures
are visible only inside `LocalFileWriter`/`BatchSink`, which therefore own those increments.
Elasticsearch, Kafka, and OTLP dispatch failures are returned to `AccessLogger`, which owns their
increments. The shared `LogType::record_drops` operation keeps both sites mapped identically.

**3. Data-plane tracing remains forbidden**

These methods run on request or connection logging paths. Do not add `tracing::*` beside the
counter: canonical access-log ownership and observability restraint require a bounded metric-only
loss signal here.

## Fix Suggestions Not Accepted

- Add a rate-limited `tracing::warn!` beside each drop. This is still request/connection-path
  tracing and duplicates the existing counter.
- Fall through to another sender after the selected healthy sender returns `Err`. The current
  policy is first healthy, with fallback based on health rather than per-entry retry.
- Add a distinct no-healthy-sender metric. It fragments the single lost-entry signal without
  changing the operator response.
- Count disabled zero-sender loggers as loss. No configured sender means logging is intentionally
  disabled, not unhealthy.

## Re-evaluation Triggers

Re-open if:

- a sender both self-counts and returns a dispatch error, because exact-once ownership must be
  restored;
- dispatch changes from first-healthy selection to fan-out, round-robin, or per-entry failover;
- a new log type is added without a corresponding dropped-counter mapping.

## Reference Cases

- observe F-003 (`tasks/observe/observe-F003-access-logger-send-silent-drop-no-metric.md`,
  2026-05-28)
- `AccessLogger::{new_with_log_type, send, send_with_meta}` in
  `edgion-gateway/src/observe/access_log/logger.rs`
- `create_async_logger` in `edgion-gateway/src/observe/logs/logger_factory.rs`
- `LogType::record_drops`, `LocalFileWriter::send`, and `BatchSink::{write_line, flush}` in
  `edgion-gateway/src/link_sys/providers/local_file/data_sender_impl.rs`
- `local-file-write-error-surfaced.md` — complementary writer-level loss handling
- `observability-review-rule.md` — data-plane tracing prohibition and metric restraint
