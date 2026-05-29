---
name: es-bulk-shutdown-cancellation
description: Read when reviewing LinkSys Elasticsearch bulk-ingest shutdown findings about the retry loop having no shutdown-aware cancellation / tokio::time::timeout not aborting the JoinHandle / old bulk task retrying detached after hot-reload.
---

# LinkSys Elasticsearch bulk-ingest shutdown cancellation (link-sys-F016)

**Status / verdict**: fixed (link-sys-F016, 2026-05-25)

When reviewing Elasticsearch `LinkSys` bulk-ingest shutdown findings of the form "`flush_batch` / `send_bulk_request` retry loop has no shutdown-aware cancellation / the 10s outer `tokio::time::timeout` in `EsLinkClient::shutdown` does not abort the JoinHandle so the task keeps running detached / on hot-reload the old bulk task retries against ES for up to `max_retries × request_timeout` (~1350s) (link-sys-F016)": **fixed (link-sys-F016, 2026-05-25)**. `send_bulk_request` (`src/core/gateway/link_sys/providers/elasticsearch/bulk.rs`) now takes a `&watch::Receiver<bool>` and returns early (`if *shutdown.borrow()`) before each backoff sleep; `flush_batch` checks the same signal before launching each sub-batch request; both are threaded from `bulk_ingest_loop`'s four call sites. `EsLinkClient::shutdown` (`client.rs`) captures `handle.abort_handle()` before `tokio::time::timeout(10s, handle)` and calls `abort_handle.abort()` on the timeout arm — `tokio::time::timeout` over a JoinHandle stops the caller waiting but never aborts the spawned task. Together these bound the post-shutdown task to ≤ one in-flight request (cooperative early-exit) plus a hard 10s abort backstop. Regression test: `test_send_bulk_request_aborts_retry_on_shutdown` (shutdown pre-signaled + unreachable endpoint + 5s backoff must return under a 2s guard). Re-reports of the unbounded-retry or non-aborting-timeout form are not accepted; verify `send_bulk_request` carries the `shutdown` param and `shutdown()` calls `abort_handle.abort()` before re-reporting.
