---
name: healthy-semantics-intentional-non-flips
description: Use when reviewing LinkSys/AccessLogger queue saturation, TCP-probed OTLP health, and accepted disk-probe tradeoffs.
status: accepted-tradeoff
finding_id: linksys-health-04
closed: 2026-06-16
knowledge_type: intentional-design
---

# `DataSender::healthy()` intentional non-flips & disk-probe tradeoffs — DESIGN-TRADEOFF

## Conclusion

The unified `DataSender::healthy()` contract (task 04, `edgion-gateway/src/link_sys/runtime/data_sender.rs`) deliberately does **not** flip health on queue saturation and accepts several bounded disk-probe imprecisions. OTLP health is now driven by a background TCP endpoint probe; export submission itself still provides no delivery acknowledgement.

Fix suggestions of the form "queue-full must mark the sink unhealthy", "OTLP healthy is frozen `true`, that violates I1", "the 90% disk default drops all logs / is too aggressive", or "fix the `current_size` undercount after disk recovery" are **not accepted** as stated — see rules below.

## Core Rationale

**1. Queue saturation (`TrySendError::Full`) stays healthy — it is backpressure, not a delivery fault.**

`LocalFileWriter::send` (`providers/local_file/data_sender_impl.rs`) distinguishes `Full` (bounded write queue full → `record_drop` only, health unchanged) from `Disconnected` (writer thread dead → `store(false)`). A momentarily full queue means the consumer is briefly behind, not that the sink cannot deliver; flipping health on it would cause flapping under bursty load and conflicts with I5 (hot path stays non-blocking, fire-and-forget). Sustained-saturation handling (spill/replay) is **task 06**, not task 04. The drop is observable via `edgion_*_log_dropped_total`. Covered by `send_stays_healthy_when_queue_is_full`.

**2. OTLP endpoint health is probe-driven; export success remains unacknowledged.**

`OtlpLogSender` delegates to its shared `OtlpLinkClient`; SDK emit remains
fire-and-forget. The owner's admitted transport probe and shared health driver
update its `Arc<AtomicBool>` using TCP reachability. A pending replacement must
not overwrite active health before that replacement is ready. A successful connect does not prove TLS negotiation or OTLP export success. Startup traffic can be shed until the probe observes recovery; integration delivery assertions must establish readiness before counting a required batch.

Historical scope: task 04 only exposed the health handle and temporarily left health true after init. The subsequent task-05 probe implementation supersedes that bridge-state description. The 2026-09-10 full integration run observed `link-sys recovered` after the collector started, and exposed the stale assumption in `run_otlp_test.sh`; this is not new acceptance of permanent healthy=true behavior.

**3. Disk probe (LocalFile) accepted imprecisions.**

- **Default-on at 90%** (`disk_usage_threshold_percent: Option<u8>`, default `Some(90)`, `0`/`None` = disabled). Sheds a LocalFile sink *before* writes fail so first-healthy can fall back. If LocalFile is the LAST sink, crossing the threshold drops entries (with a metric) — operators who want to write to the last byte raise the threshold or set `0`. Documented on the config field.
- **No hysteresis.** A disk oscillating exactly at the integer boundary flips health (one transition WARN per probe, cadence ≥1s / default 30s). Bounded and rare; a margin would only be added if churn is observed in practice.
- **`f_bavail`-based usage** excludes root-reserved blocks, so usage reads marginally high (conservative shedding) — correct for a non-root writer.
- **`current_size` undercount after disk-full→recover:** bytes retained by `BufWriter` after a failed flush, when later landed by `retry_flush_for_recovery`/a recovered batch, are not re-added to `current_size`, so size rotation can overshoot by up to one buffer (~8 KB vs a 100 MB default `max_size`). Documented best-effort-pessimistic behavior.

**4. Recovery IS live (this is NOT a tradeoff — it is required and implemented).** A shed LocalFile gets no traffic, so the writer thread uses `rx.recv_timeout(idle_tick)` (`idle_tick = check_interval.max(1s)`) to re-probe disk and retry a flush even when idle. Do not "simplify" this back to `rx.recv()` — that reintroduces the permanent stuck-unhealthy bug (review finding B-1).

## Fix Suggestions Not Accepted

- "`send` should mark unhealthy when the queue is full" — flapping under load, violates I5; saturation handling is task 06.
- "OTLP `healthy` is frozen `true`, drive it from `emit`" — `emit` is fire-and-forget; no local signal exists. Driven by the shared owner's canonical probe; a thin sender does not own a health handle.
- "Lower/raise the 90% default" or "never shed the last sink" — operator-tunable (`0` disables); shedding-before-failure is the point.
- "Add disk hysteresis now" — premature; transitions are already gated to true edges, default 30s cadence.
- "Make `current_size` exact across failed-then-recovered flushes" — unbounded bookkeeping for a ≤8 KB drift vs a 100 MB rotation size.
- "Replace `recv_timeout` with `recv` to simplify" — reintroduces B-1 (shed sink never recovers).

## Re-evaluation Triggers

Re-open only if:

- Task 06 (FailedCache / backpressure) lands and changes how queue saturation should surface in health.
- Endpoint probing or export acknowledgement changes. Task 05 has landed; its trigger already superseded the historical OTLP bridge state above.
- Operators report real disk-threshold flapping (→ add hysteresis) or last-sink log loss surprises (→ revisit default).

## Reference Cases

- Contract: `tasks/todo/linksys-health-degrade/00-overview.md` invariants I1/I4/I5/I7, plus the trait doc `edgion-gateway/src/link_sys/runtime/data_sender.rs` `healthy()`. (The per-task spec `04-unify-healthy-semantics.md` was removed at task cleanup; design rationale now lives in the plan below.)
- Plan: `docs/superpowers/plans/2026-06-16-linksys-unify-healthy-semantics.md`.
- Implementation: `edgion-gateway/src/link_sys/providers/local_file/data_sender_impl.rs`, `.../providers/otlp/log_sender.rs`, `.../runtime/data_sender.rs`, `edgion-resources/src/output.rs`.
- Tests: `send_marks_unhealthy_when_writer_thread_is_gone`, `send_stays_healthy_when_queue_is_full`, `batch_sink_*` (flush-driven flip / recover / disk flip / idle recovery), `send_skips_unhealthy_and_falls_through_to_next`.
