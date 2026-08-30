---
name: retry-counter-metric-declined
description: Proposal to add a gateway_retries_total{reason} Prometheus counter at the four retry decision sites was declined by design — retry rate (by reason) is computed from access-log aggregation over backend_context.upstreams[].err, per the data-plane metrics rules. Reject re-reports proposing per-retry counters; point reviewers at the explicit per-attempt reason labels instead.
---

# route-03 (per-try-timeout deep review) — retry Prometheus counter declined by design

**Status:** ❌ declined (by design) · 2026-06-10
**Origin:** per-try-timeout feature deep review (ops/resilience reviewer), routed via `tasks/todo/route-03-retry-observability.md`

## Conclusion

The proposed `gateway_retries_total{reason="per_try_timeout|connect_fail|status|reuse"}`
counter contradicts the project's observability design
(`skills/03-coding/observability/01-metrics.md`): data-plane exceptions go
through the access log (`AccessLogEntry.diagnostics` + `EdgionErrorCode`), finer
observation goes through access-log aggregation, and the explicit pre-add
check — "can a field already in the access log express it via aggregation?" —
answers yes here. The access log emits one `backend_context.upstreams[]` entry
per attempt; retry count is `upstreams.length - 1` and every Edgion-decided
retry site stamps an explicit reason label into the attempt's `err` array
(`RetryOnStatus`, `ConnectionReuseRetry`, etype strings for connect failures and
per-try timeouts; Pingora-internal `Decided(true)` retries, e.g. h2 GOAWAY,
are intentionally unlabeled). "Retry rate is spiking, by reason" is a log pipeline
aggregation (Loki / Elasticsearch / ClickHouse), not a Prometheus counter.

## Fix Suggestions Not Accepted

- "Add `gateway_retries_total{reason}` incremented at `pg_fail_to_connect.rs`,
  `pg_upstream_response_filter.rs`, `pg_error_while_proxy.rs` Case 1.5/2."
  Rejected: access-log aggregation expresses it; per-reason label cardinality
  on a hot-path counter is exactly what the metrics rules restrain.
- Any variant ("retries_total without labels", "only per-try-timeout retries")
  fails the same pre-add check. The fleet alert belongs in the log pipeline.
