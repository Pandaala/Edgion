# gw-infra2-02: Access-log ES backfill starves live access logs for its whole duration and can lose already-"backfilled" lines; the in-gateway replay feature is the wrong layer

- **Category**: design-error (with an over-design recommendation)
- **Severity**: P2 (worth doing)
- **Scope**: local
- **Location**: `edgion-gateway/src/observe/access_log/backfill.rs:99-170` (`run_backfill`),
  `edgion-gateway/src/link_sys/providers/elasticsearch/client.rs:276-307` (`send_bulk` vs `try_send_bulk`),
  `edgion-gateway/src/link_sys/providers/elasticsearch/log_sender.rs` (`send` → `try_send_bulk`),
  `edgion-gateway/src/observe/access_log/logger.rs:56-68` (drop on send error, no fall-through),
  `edgion-gateway/src/link_sys/providers/elasticsearch/bulk.rs:132-215,404-517` (serial ingest loop, drops on 4xx / ambiguous ack / retry exhaustion),
  `edgion-gateway/src/api/mod.rs:178-206` (admin handler)

## Summary
`POST /api/v1/access-log/backfill` replays fallback files into the **same** bounded ES bulk channel that
live traffic uses, with an awaiting `send` that always holds a queued waiter on that channel. Live
access logs use `try_send`, and because ES is healthy (backfill requires it) the logger does not fall
back — every live entry that meets a full channel is dropped. During a backfill the channel is full by
construction, so live access logging degrades to near-total loss for as long as the replay runs
(minutes to hours for a large outage file). Separately, files are renamed `.backfilled-*` at
queue-entry, while the bulk loop silently drops batches on 4xx, ambiguous acknowledgements and retry
exhaustion, so "worst case is duplication, never data loss" does not hold.

## Current behavior (evidence)
- Backfill path: `run_backfill` → `client.send_bulk(line).await` (`backfill.rs:130`). `send_bulk`
  (`client.rs:276-291`) does `try_send`, and on `Full` falls back to `sender.send(doc).await`, i.e. it
  parks as a waiter on the tokio bounded channel's semaphore.
- Live path: `EsLogSender::send` → `try_send_bulk` (`client.rs:299-307`) → `try_send`, error on full.
  `AccessLogger::send` (`logger.rs:56-68`) counts a drop and returns; it does not try the fallback
  sender because the ES sender is `healthy()`.
- Tokio's bounded mpsc hands released permits to queued waiters first; `try_send` only sees permits
  that no waiter claimed. While backfill keeps one waiter queued, nearly every slot freed by the ingest
  loop goes to backfill. The ingest loop (`bulk.rs:146-213`) is serial: it awaits `flush_batch`
  (including retries with backoff up to 30 s) without draining `rx`, so the channel stays at capacity
  the entire time.
- Loss after rename: `mark_backfilled` runs once all lines are *queued* (`backfill.rs:154-166`). The
  bulk loop then drops data without retry on 4xx (`bulk.rs:464-467`), on an unreadable/ambiguous bulk
  receipt (`bulk.rs:452-457`, `bulk.rs:416-422`), plus permanently-failed items in a partial-success response (`bulk.rs:428-435`), on retry exhaustion (`bulk.rs:477-487`), and on
  shutdown (`bulk.rs:494-515`). The just-recovered ES that backfill targets is precisely the cluster
  most likely to hit these paths. None of these are counted in the backfill summary.
- Reading is blocking `std::fs` + `BufReader::lines()` inside the async admin handler
  (`backfill.rs:117-130`), on the shared runtime.
- Backfilled documents land in today's index (documented in `docs/en/ops-guide/observability/access-log.md:148`),
  breaking per-day ILM/retention for exactly the replayed data.

## Industry comparison
Envoy, NGINX, Kong and APISIX do not replay their own log files. The proven pattern is: gateway writes
locally / to a bounded async sink with a drop counter, and a log shipper (Fluent Bit / Vector /
Filebeat) owns durable buffering, checkpointed replay (file offsets), retry with dead-letter, and
index routing by event timestamp. APISIX's batch-processor, the closest in-gateway analogue, retries
the *same* batch with a bounded retry count and never competes with live entries for a queue
reserved for replay.

## Impact
An operator following the documented recovery procedure after an ES outage causes a *second*,
self-inflicted access-log outage for live traffic (visible only as `edgion_access_log_dropped_total`),
and may still lose part of the outage window that the procedure was meant to recover, while the
fallback file has already been marked as done.

## Suggested direction
Options:
1. **Recommended (simplify)**: remove the backfill endpoint. Keep the LocalFile fallback (or just
   LocalFile as the always-on sink) and document a shipper (Fluent Bit/Vector tail with checkpoint and
   `@timestamp`-based index routing) as the recovery path. This deletes `backfill.rs`, the admin route,
   the single-flight guard, the rename protocol, and the rotation-name matcher coupling.
2. **If kept**: give backfill its own low-priority path — a separate small bounded channel (or a
   token-bucket rate limit well below ingest capacity) so live `try_send` never competes with a queued
   waiter; only mark a file done after the bulk loop acknowledges every batch from it (per-file ack
   counter); count 4xx/ambiguous/exhausted drops into the summary; index by the document's
   `@timestamp`; read the file with `tokio::fs`/`spawn_blocking`.

## Notes
`skills/04-review/link-sys/intentional-design/access-log-fallback-backfill-tradeoffs.md` accepts (1)
health-based fallback, (3) at-least-once duplication and (4) queue-entry marking, and states "worst case
is duplication, never data loss" with crash as the only loss case. New evidence not considered there:
(a) live-traffic starvation caused by the backfill waiter on the shared channel while ES is healthy, and
(b) non-crash loss paths in the bulk loop (4xx, ambiguous ack, retry exhaustion) after the file is
already renamed. The page's re-evaluation trigger is therefore met on correctness grounds, not on an
exactly-once requirement. No related task under `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P2 unchanged.
- `elasticsearch/client.rs:276-293` `send_bulk` falls back to an awaiting `sender.send(doc)` on `Full`, while live logs use `try_send_bulk` (`:299-308`). Tokio's bounded channel hands freed permits to queued waiters first.
- `access_log/logger.rs:56-68` drops the entry and records the drop when the healthy sender's send fails. It does not fall through to another sender.
- `backfill.rs:117-170` uses blocking `std::fs` with `BufReader::lines()` inside the async handler, and `mark_backfilled` runs after queue-entry.
- `elasticsearch/bulk.rs:452-487` returns without retry on an ambiguous ack, on 4xx, and on retry exhaustion ("items lost"). This contradicts "never data loss" in `link-sys/intentional-design/access-log-fallback-backfill-tradeoffs.md:42`, which lists only crash loss. That page needs correcting together with the fix.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P2 unchanged.
- `observe/access_log/` (`backfill.rs`, `logger.rs`), `elasticsearch/client.rs`, `log_sender.rs` and `bulk.rs` are unchanged between feature-08-30 and HEAD; the only ES change is a 1 MiB cap on cluster-health responses in `elasticsearch/ops.rs`.
- `send_bulk` (`client.rs:276-293`) still parks an awaiting waiter on the shared channel while live logs use `try_send_bulk`; `bulk.rs:452-487` still drops on ambiguous ack, 4xx and retry exhaustion after `mark_backfilled`.
- `link-sys/intentional-design/access-log-fallback-backfill-tradeoffs.md` is unchanged and still claims "never data loss" beyond crash.
