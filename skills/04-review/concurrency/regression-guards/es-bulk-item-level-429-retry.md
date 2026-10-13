---
name: es-bulk-item-level-429-retry
description: Use when reviewing LinkSys Elasticsearch Bulk HTTP-success responses with item-level 429 failures, response/input correlation, retry budgets, or later-log queueing.
knowledge_type: regression-guards
---

# LinkSys Elasticsearch Bulk item-level 429 retry

## Conclusion and scope

`send_bulk_request` in
`edgion-gateway/src/link_sys/providers/elasticsearch/bulk.rs` must inspect a
successful HTTP Bulk response's per-item results. An item status of 429 is an
explicit overload rejection and is retried after the configured exponential
backoff. Only those rejected documents are rebuilt into the next NDJSON body;
acknowledged successes and other item failures are not resubmitted.

This rule covers the Gateway's Elasticsearch LinkSys access-log sink, which emits
only `index` actions. It does not define retry semantics for arbitrary
`create`, `update`, or `delete` Bulk clients.

## Rationale and evidence

Elasticsearch and OpenSearch specify that Bulk response items are returned in
request order. `classify_bulk_items` relies on that order only after verifying an
exact item-count match and that every result is an `index` result. It records the
indices of 429 items so the caller can select the corresponding original documents
without copying their bodies. Protocol authority:
[Elasticsearch Bulk API](https://www.elastic.co/docs/api/doc/elasticsearch/operation/operation-bulk),
[Elasticsearch indexing guidance](https://www.elastic.co/guide/en/elasticsearch/reference/current/tune-for-indexing-speed.html),
and [OpenSearch Bulk API](https://docs.opensearch.org/latest/api-reference/document-apis/bulk/).

Transport failures, HTTP 5xx responses, and item-level 429 responses share one
`EsBulkConfig.max_retries` budget. The budget is not reset when a partial response
shrinks the pending set. `EsBulkConfig.backoff` remains the first delay and doubles
to the existing 30-second cap. While the single bulk loop awaits retry, later log
entries remain in the existing bounded `channel_size` mpsc queue up to its capacity.
Once full, new `try_send_bulk` submissions are dropped and counted; they do not fall
through to the fallback. No second retry queue or implicit disk cache exists.

## Alternatives and limits

- Retrying the complete original batch after a parsed partial response was rejected:
  it would duplicate documents that Elasticsearch already acknowledged.
- Hard-coding a 100 ms delay was rejected: the operator-facing `backoffMs` field
  already owns the initial delay and defaults to 1000 ms.
- Adding jitter was left outside this correction. Elasticsearch recommends
  randomized exponential backoff, but the existing request-level retry loop is
  deterministic and this finding does not redesign that timing policy.
- A response with a mismatched item count or a non-index item is ambiguous and is
  not retried. A JSON parse failure retains the pre-existing no-retry behavior for
  the same duplicate-avoidance reason.
- Network errors and HTTP 5xx responses still retry the current pending set. Their
  pre-existing at-least-once duplicate risk is unchanged when the server processed
  a request but the client did not receive an item-level acknowledgement.
- Permanent item failures are reported and dropped. Durable failed-document storage
  remains unimplemented; a configured access-log fallback receives new entries only
  after the Elasticsearch connector becomes unhealthy.

When an item-level 429 later succeeds, the connector stays healthy. If the shared
retry budget is exhausted, the bulk path marks it unhealthy so a configured
first-healthy fallback can receive subsequent entries. The retry backoff races its
sleep against a cloned shutdown receiver, so shutdown suppresses the next request
even when it arrives during the wait; `EsLinkClient::shutdown` retains its 10-second
abort backstop. Retry bodies remain subsets of a sub-batch already bounded by
`max_body_bytes`.

## Bounded acknowledgement reception (ARCH-027)

The provider resolves LinkSys `bulk.maxResponseBytes` once with a 1 MiB default
and inclusive 1-byte to 8-MiB clamp. Its resolved value is in the runtime
fingerprint, so changed effective limits replace the worker generation. The
opaque CRD config preserves this operator field. The Gateway canonical collector
lives in `util/http_body.rs`; DSL, Wasm/OCI, OIDC, managed outbound and ES share
its receiving implementation while retaining caller-owned caps and outcomes.

Declared oversized receipts are rejected before reading; streamed accumulation
also stops at the cap. No truncated prefix is parsed. HTTP-success cap/read/parse
failures are ambiguous, do not replay and retain existing healthy/retirement
fencing. A rejected oversized receipt can therefore lose item-level 429 retries.
The minimal projection retains only errors/items/index/status; omitted or wrongly
typed unused metadata is intentionally ignored. Required relevant fields and JSON
syntax remain checked, and errors=false retains its existing classifier bypass.

The limit bounds input, not total heap or process memory. Vec capacity, delivered
chunks, projected items and serde scratch coexist. Locked serde_json 1.0.151 skips
ignored nested metadata iteratively with delimiter scratch rather than retaining
Value trees; its ordinary typed recursion limit is not an ignored-value depth
limit. Re-evaluate this source-derived feasibility if the dependency changes.

## Verification and re-evaluation

Historical verification for the earlier item-level retry correction (not fresh
ARCH-027 verification):

- `cargo test -p edgion-gateway link_sys::providers::elasticsearch::bulk::tests --lib`
  — 16 passed, 0 failed.
- `test_send_bulk_request_retries_only_429_items` verifies that the second wire
  request contains the rejected document and excludes the acknowledged document.
- `test_send_bulk_request_degrades_after_429_retry_budget_is_exhausted` verifies the
  shared attempt bound and unhealthy transition.
- `test_retry_backoff_is_cancelled_after_wait_starts` first polls the 30-second
  backoff helper to pending, then verifies that shutdown cancels it promptly.
- Classification tests cover permanent errors, stable retry order, item-count
  mismatch, and unexpected action results. The pre-existing shutdown regression
  remains in the same targeted test set.

Current ARCH-027 verification completed so far:

- `cargo test -p edgion-resources --lib resources::link_sys::elasticsearch::tests`
  passed 15 tests, including maxResponseBytes serialization and opaque CRD
  preservation. This does not establish Gateway receiving or retry behavior.
- Source comparison preserved existing apply (9), bulk (19), configuration
  mapping (17) and Resources Elasticsearch (14) test identities. It added one,
  seven, one and one respectively, plus four collector cases. This comparison
  is source evidence, not a freshly executed baseline test listing.
- Scoped formatting of the changed Rust files and `git diff --check` passed.
- `cargo fmt --all` and the Gateway Elasticsearch test listing were attempted
  and blocked by conflict markers committed in unrelated baseline Gateway
  modules. Gateway receipt, collector, fingerprint, retry and cancellation tests
  have not run. The current change therefore has not completed verification or
  independent acceptance; historical results above cannot substitute for them.

Re-evaluate this rule if the sink emits Bulk actions other than `index`, gains stable
document IDs or a durable failed-document cache, changes the meaning of
`maxRetries`, adopts randomized backoff, or parallelizes the currently serial bulk
loop.

## Resolution references

- `elasticsearch-bulk-item-retry-semantics` — fixed in the Gateway Elasticsearch
  LinkSys Bulk path; the verified task record is removed at closure.
