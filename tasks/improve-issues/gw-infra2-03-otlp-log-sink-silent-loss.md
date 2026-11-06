# gw-infra2-03: OTLP log sink loses records invisibly — SDK queue overflow and export failures bypass Edgion's drop accounting, and its queue is a hard-coded 2048

- **Category**: design-error
- **Severity**: P2 (worth doing)
- **Scope**: local
- **Location**: `edgion-gateway/src/link_sys/providers/otlp/client.rs:96-115` (BatchLogProcessor config), `client.rs:287-295` (`send_with_meta` returns `Ok` after `emit`), `client.rs:31-70` (exporter wrapper), `edgion-gateway/src/link_sys/providers/otlp/log_sender.rs:44-73` (`emit_record`), `edgion-gateway/src/observe/access_log/logger.rs:56-87` (drop metric only on `Err`)

## Summary
Every other access-log sink reports loss through the canonical `edgion_*_log_dropped_total` counter:
LocalFile and Kafka reject with `try_send`/budget errors, ES with `try_send_bulk` errors, and
`AccessLogger` increments the per-log-type drop counter on `Err`. The OTLP sink hands records to the
OpenTelemetry SDK `BatchLogProcessor` and always returns `Ok`. When the SDK's internal queue (fixed at
2048 records, not configurable) is full, the SDK drops the record internally; when an export fails or
times out, the SDK discards the whole batch (up to 512 records). Neither event reaches Edgion's drop
metric, and `healthy()` is driven by a connectivity probe, so a reachable-but-slow collector loses logs
while every Edgion signal says the sink is healthy and lossless.

## Current behavior (evidence)
- `client.rs:104-109`: `BatchConfigBuilder::default().with_max_queue_size(2048).with_max_export_batch_size(512).with_scheduled_delay(1s)`;
  no field of the OTLP LinkSys config feeds these values (contrast `KafkaClient::queue_budget`
  `max_pending_records/bytes` and `channel_size`, ES `bulk.channel_size`, LocalFile `queue_size`).
- `client.rs:287-295`: `send_with_meta` checks `healthy()`, calls `emit_record`, returns `Ok(())`.
  `emit_record` (`log_sender.rs:53-73`) calls `logger.emit(record)`, which in opentelemetry_sdk (0.31 at the time; now 0.33.1)
  enqueues with a non-blocking `try_send` and, when full, only bumps the SDK-internal dropped count
  (reported via the SDK's own internal diagnostics at shutdown), never to the caller.
- The exporter wrapper (`client.rs:48-62`) maps a timeout to `OTelSdkError::InternalFailure`; the batch
  processor then drops the batch. No retry and no Edgion metric.
- `AccessLogger::send_with_meta` (`logger.rs:76-87`) only records drops when the sender returns `Err`,
  so the OTLP path can never contribute to `edgion_access_log_dropped_total`.
- At typical gateway rates (tens of thousands of requests/s per replica) a 2048-record queue with a
  single in-flight gRPC export is a small buffer: one collector GC pause or a 1–2 s export stall
  overflows it.

## Industry comparison
- Envoy's OpenTelemetry access logger buffers with `buffer_size_bytes` / `buffer_flush_interval` (both
  configurable) and exposes per-logger stats (`logs_written`, `logs_dropped`); gRPC stream failures are
  counted.
- The OpenTelemetry Collector's exporters expose `otelcol_exporter_send_failed_log_records` and
  `otelcol_exporter_enqueue_failed_log_records`; queue size is configurable (`sending_queue.queue_size`).
- Kong's OpenTelemetry plugin exposes `queue.max_entries` / `max_batch_size` and logs/accounts drops.

## Impact
OTLP is the sink Edgion recommends for vendor-neutral pipelines, yet it is the only one where log loss
is unobservable from Edgion's metrics, and the only one whose buffering cannot be sized for the
gateway's throughput. Alerting on `edgion_access_log_dropped_total` gives false assurance.

## Suggested direction
1. Put an Edgion-owned bounded queue in front of the SDK (as Kafka/ES do) or implement a custom
   `LogProcessor` that does `try_send` into a sized channel and returns an error on full, so
   `AccessLogger` counts the drop; alternatively wrap `emit` with a counter-based admission
   (`pending < max`) that is decremented by the exporter.
2. Count failed/timed-out exports (records in the batch) into the same per-log-type drop counter from
   the exporter wrapper (`client.rs:48-62`), and feed sustained export failure into `healthy()` so the
   fallback sink engages, matching ES/Kafka behavior.
3. Expose `maxQueueRecords`, `maxExportBatchSize`, `scheduledDelay` on the OTLP LinkSys config with the
   current values as defaults.

## Notes
Checked `skills/04-review/link-sys/regression-guards/otlp-provider-ownership-and-egress.md` (ownership,
retirement and egress — does not discuss loss accounting) and
`skills/04-review/link-sys/intentional-design/access-log-fallback-backfill-tradeoffs.md` (health-based
fallback is accepted; this finding is about loss being unaccounted, not about fallback routing). Not
tracked under `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P2 unchanged.
- `otlp/client.rs:104-109` hard-codes `with_max_queue_size(2048)`, `with_max_export_batch_size(512)` and a 1 s delay. No LinkSys config field feeds these values.
- `otlp/client.rs:287-295` `send_with_meta` checks only `healthy()`, calls `emit_record` and returns `Ok(())`, so `AccessLogger` never records an OTLP drop.
- The `dropped` oneshot in `client.rs:38,81,96` is an exporter-release signal for shutdown, not a loss counter. No Edgion metric counts SDK queue overflow or failed exports.
- `link-sys/regression-guards/otlp-provider-ownership-and-egress.md` covers ownership, retirement and egress only.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P2 unchanged.
- OpenTelemetry was upgraded to 0.33.1 and the exporter now uses `RetryPolicy::disabled()` (`otlp/transport.rs:159-160`), but `otlp/client.rs:104-108` still hard-codes queue 2048 / batch 512 / 1 s, and `send_with_meta` (`client.rs:287-295`) still returns `Ok(())` after `emit_record`.
- `link-sys/regression-guards/otlp-provider-ownership-and-egress.md` (new "SDK 0.33.1 upgrade boundary") states that queue limits and batch size are unchanged and adds no loss accounting; `observe/access_log/logger.rs` is unchanged, so OTLP loss still never reaches `edgion_access_log_dropped_total`.
