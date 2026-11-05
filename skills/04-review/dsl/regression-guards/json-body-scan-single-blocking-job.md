---
name: json-body-scan-single-blocking-job
description: Use when reviewing DSL json_get/json_select over a request Body, ReadBodyScanner, or BodyStorageView::scan_read.
knowledge_type: regression-guards
---

# DSL Body JSON scans run as one pull-based scan-lane job

## Conclusion and scope

`json_get(Body)` and `body.json_select` parse the request body through the
request-body-owned pull interface `ReadBodyScanner`. `DslBodyOps::scan_read_with`
reserves the shared `BodyScanBudget`, then `RequestBodyState::scan_read` →
`BodyStorageView::scan_read` runs the
serde parser in exactly one admitted job on the request-body scan lane (the
shared 32-permit scan limiter). The job reads the immutable `BodySnapshot`
through `SnapshotRead`, an `io::Read` over the same `SnapshotChunks` cursor used
by push scans: memory segments are borrowed, disk segments are read into one
reusable budgeted scratch buffer, and both are fetched in 64 KiB chunks.

Do not reintroduce a per-scan OS thread, a channel between storage and the
parser, a per-chunk owned copy, a second permit for Body JSON scans, or an
async-reader `block_on` bridge. JSON policy (path selection, capture limits,
duplicate-key replacement, `Missing`/`TooLong`) stays in
`plugins/http/dsl/lang/body_scanners.rs`; snapshot traversal stays in
`request_body/storage.rs`.

The owned-string `json_get(Str)` path (`JsonPathScanner::scan_owned`) is out of
scope and still uses its own JSON permit plus the generic scan lane.

## Rationale and evidence

Before this change each Body JSON scan acquired a JSON permit, spawned a new
`edgion-json-path`/`edgion-json-select` OS thread fed by `mpsc::sync_channel(2)`,
then acquired a second permit for a `spawn_blocking` push scan that copied every
chunk into a `Vec`. The thread existed only to adapt serde's pull `Read` to the
push-based `RawBodyScanner`. Parse termination was observed only on the next
failed `send`, so the number of charged chunks after a malformed document was
racy.

Current semantics preserved by the cursor/reader:

- Cancellation and scan-budget checks happen before every chunk fetch; a chunk
  is charged when fetched, including the chunk whose consumption reveals parse
  termination. A parser that stops early is charged only for fetched chunks.
- A chunk error (budget exhaustion, cancellation, storage failure) is retained
  by `SnapshotRead` and returned after `scan`, overriding the parser's
  `Missing`, so `ScanLimitExceeded` and storage-failure policy are unchanged.
  Errors leave the DSL reservation unsettled (no refund).
- `deserializer.end()` still reads to EOF, so full-document validation and the
  trailing-data `Missing` result are unchanged.
- A known-empty body runs the scanner on `io::empty()` through the blocking
  scan lane (parity with `scan_raw`) with zero bytes charged; a not-buffered
  body still reports `NotBuffered`.

Concurrency decision: Body JSON scans now hold one permit on the generic
32-permit scan lane shared with every other blocking body scan. Previously each
scan held both a JSON permit and a scan permit (plus an OS thread), so effective
Body JSON concurrency was already bounded by 32; the change removes the separate
JSON cap without reducing that capacity.

## Alternatives and limits

- Keeping the thread and pooling it was rejected: it keeps two blocking contexts
  and the chunk copy.
- `block_on` over the async `RequestBodySnapshotReader` was rejected by the task
  scope; it would nest runtime blocking inside the scan lane.
- serde_json's `IoRead` pulls one byte per `read` call, so `SnapshotRead::read`
  delegates to the `&[u8]` reader (keeping std's single-byte fast path); budget
  and cancellation checks run only per chunk fetch.

## Verification and re-evaluation

Verified on 2026-11-05 with `cargo test -p edgion-gateway --lib -- request_body::
plugins::http::dsl` (all passed), including:
`scan_read_streams_memory_disk_and_mixed_snapshots_off_the_caller`,
`scan_read_charges_only_fetched_chunks_when_the_parser_stops_early`,
`scan_read_budget_exhaustion_overrides_the_parser_result`,
`cancelled_scan_read_fails_the_next_chunk_fetch` (deterministic: the scanner
waits until its future is dropped, then the next fetch must fail),
`body_facade_failed_pull_scan_keeps_the_scan_reservation`,
`known_empty_pull_scanner_reads_an_empty_body_without_charge`,
`truncated_and_malformed_json_bodies_return_missing`,
`json_path_scan_over_spilled_storage_keeps_budget_and_validation_policy`, and the existing spilled
VM tests in `dsl/mod_test/body_value.rs`. No micro-benchmark was recorded; the
thread elimination is structural. No test injects a disk read failure into a
pull scan; storage-failure precedence relies on the same retained-error path
covered by the budget and cancellation tests.

Re-evaluate if the scan-lane permit count changes, if another Body parser needs
`io::Read`, or if Body JSON parsing moves off the blocking pool.

## Resolution references

- `dsl-07-json-scan-transport-ownership` (P2 task; the untracked task file was
  removed on completion), fixed 2026-11-05.
