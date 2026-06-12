---
name: access-log-all-senders-unhealthy-metric
description: AccessLogger::send now increments edgion_access_log_dropped_total when no sender reports healthy, so "all writers down / sender init failed" is observable via the same per-log-type counter used for queue-full and write-failure drops. Reject re-reports claiming the all-unhealthy path is silently dropped, and reject proposals to add a data-plane tracing::warn! alongside the metric.
---

# observe F-003 (access_log) — all-senders-unhealthy path increments the dropped counter

**Status:** ✅ fixed · 2026-05-28
**Severity:** Medium (BadLogic) — silent log loss when every registered sender reports `healthy()==false`

## Conclusion

`AccessLogger::send` (`edgion-gateway/src/observe/access_log/logger.rs`) now calls
`global_metrics().access_log_dropped()` after the sender loop exhausts without finding a healthy
one, so a "writer task died / sender failed to initialize / channel was never created" state is
visible on the same `edgion_access_log_dropped_total` counter used for queue-full
(`local_file/data_sender_impl.rs:191`), background write-failure (F-020), and integration-test
`AccessLogStore::store` failure (`pg_logging.rs:84`). The dispatch loop itself is unchanged — the
first healthy sender still wins and the `let _ = sender.send(...)` discard on a healthy sender
is retained because the only registered sender (`local_file`) already self-counts both queue-full
and write-failure drops internally.

Fix suggestions of the form "also emit `tracing::warn!` once per cadence on the all-unhealthy
path, mirroring `ssl_log.rs`" are **not accepted** — `AccessLogger::send` is reached from the
data-plane hot path (`pg_logging.rs:64`, `redirect_http.rs:207`, `tls/proxy.rs:435`,
`tls/passthrough_proxy.rs:435`, `pg_connected_to_upstream.rs:55`); `tracing::*` is forbidden there
by the "Zero Tracing on the Data Plane" iron rule in `observability-review-rule.md`. The premise
of the suggestion also misreads the precedent: `ssl_log.rs:48-50` itself only increments the
metric and does NOT emit `tracing::warn!` on the drop path.

## Core Rationale

**1. Counter-only is the project's data-plane signal-loss pattern**

The same `edgion_*_log_dropped_total` counter family covers every "a log line was lost" case
across access / ssl / tcp / tls / udp loggers. Reusing it for the all-unhealthy case answers the
operator's single question — "are we losing log lines, and at what rate" — without doubling the
alerting surface. This is consistent with F-020's decision to surface background write-failures on
the same counter rather than minting a new write-error metric.

**2. The all-unhealthy case is reachable but not handled by F-020**

F-020 makes a single `LocalFileWriter` surface its own write failures via the dropped counter,
but only while the background thread is alive. If the writer task panics or its `init` returns
`Err` such that `healthy()` flips to `false` (or the sender is never registered as healthy at
all), every subsequent `AccessLogger::send` falls through the loop with no signal. F-003 closes
that gap at the dispatcher level. The two fixes compose; neither subsumes the other.

**3. Data-plane tracing is forbidden, including "once per cadence"**

All five call sites of `AccessLogger::send` are on the request/connection completion path. Iron
Rule 3 (`observability-review-rule.md` · "Zero Tracing on the Data Plane") forbids any
`tracing::*` macro on this path regardless of level or rate-limit framing. Operators detect
sustained loss by alerting on the existing counter (e.g., `rate(edgion_access_log_dropped_total[5m]) > 0`),
not by scraping process stdout.

## Fix Suggestions Not Accepted

- "Add a `tracing::warn!` (once per cadence / per N drops) next to the counter on the all-unhealthy
  path, mirroring `ssl_log.rs`." Rejected on two grounds: the cited precedent does not actually
  emit `tracing::warn!`, and the call site is on the data-plane hot path where Iron Rule 3 forbids
  any `tracing::*` regardless of level.
- "Replace the `let _ = sender.send(data).await` on the healthy-sender branch with a `Result`
  propagation + per-call counter increment." Not accepted — the only registered sender
  (`local_file`) already increments the same counter internally on both `try_send` queue-full
  (`data_sender_impl.rs:191`) and background `writeln!` failure (F-020), so a dispatcher-level
  increment would double-count those drops. The discard is intentional and stays.
- "Add a separate `access_log_no_healthy_sender_total` metric so the all-unhealthy case is
  distinguishable from queue-full / write-failure." Not accepted — the operator question is "are
  we losing log lines", and the alerting threshold is the same in all three cases. A separate
  metric only fragments dashboards.
- "Flip `healthy()` based on the dispatch outcome so failover can route around a dead writer."
  Out of scope — same reasoning as F-020 (`local-file-write-error-surfaced.md` ·
  "Fix Suggestions Not Accepted"): conflating transient runtime faults with the startup-style
  health bool risks oscillation and routing-to-nowhere in a single-sender setup. If failover is
  ever wanted, track it as a separate task.

## Re-evaluation Triggers

Re-open only if:

- A second non-`local_file` sender is added whose `send()` failure path does NOT self-count on
  `*_log_dropped` — then revisit whether the dispatcher should also count per-sender failures.
- The dispatch policy changes from "first healthy wins" to fan-out / round-robin — the all-unhealthy
  branch and its counter call would need to move accordingly.
- A future requirement genuinely needs writer-level failover (then revisit the `healthy()` flip
  decision as its own task).

## Reference Cases

- observe F-003 (`tasks/observe/observe-F003-access-logger-send-silent-drop-no-metric.md`, 2026-05-28)
- `local-file-write-error-surfaced.md` — complementary fix at the writer level
- `observability-review-rule.md` — Iron Rule 3 (Zero Tracing on the Data Plane)
- `edgion-gateway/src/observe/metrics/registry.rs:178` — `access_log_dropped()` counter definition
- `edgion-gateway/src/observe/logs/ssl_log.rs:48-50` — sibling pattern (metric-only on drop, no `tracing::warn!`)
