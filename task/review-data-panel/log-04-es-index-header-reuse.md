# Evaluate Elasticsearch index-action reuse across a flush

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Inspected baseline: `c820cc470`.

## Evidence and candidate

`elasticsearch/bulk.rs::flush_batch` generates the current index name once, then
serializes ndjson_header_line to calculate per-document overhead and drops that
String. Each sub-batch enters send_bulk_request_using; each retry constructs a
new body through build_ndjson_body, serializing the same header again. The recent
borrowed typed serializer removes the temporary Value tree but not these repeated
header serializations. The final body still needs repeated header bytes per doc.

A single serialized index-action owner per flush could serve partition accounting,
all sub-batches and retries. Preserve the once-per-flush UTC date evaluation even
across midnight or backoff; preparing per provider generation would freeze the
wrong date. Keep one serde escaping implementation and distinguish encoded header
input from a raw index name in any helper interface.

## Allocation and lifetime tradeoff

Today the raw generated index name remains owned by flush_batch, but each encoded
header is temporary inside body construction and is dropped before send(body).await.
A hoisted encoded String would survive network requests, response receipt reading,
and retry delays. Keeping both representations increases retained storage; dropping
the raw name after encoding is a separate alternative to measure. JSON escaping
can expand a control byte into six ASCII bytes, so raw and encoded capacities
cannot be assumed equal. No net-memory, latency or throughput change is measured.

The provider EsIndex schema uses String prefix/datePattern without a dedicated
length bound, and validate_date_pattern checks formatting capability, not output
length. max_body_bytes bounds admitted wire bodies for nonempty sub-batches; it
is not a general header-allocation cap because construction precedes partitioning.
If all documents are oversized, no sub-batch send await occurs. Do not introduce
new rejection limits merely to justify this optimization.

## Decision and acceptance

Measure serialization count/CPU, allocation calls and live capacities for normal
short names, escaping-heavy and long operator names, one/many sub-batches, successful
requests, 5xx retries and item-level 429 retries. Compare retaining both strings
with releasing the raw name. A benchmark of serialization alone cannot establish
whole-worker peak or retained memory while awaiting a slow peer.

Require byte-identical bodies and overhead at exact partition boundaries, ordered
partial retry subsets, unchanged shared retry budget, and first/current date
semantics. Preserve invalid-date batch drop, oversized-doc handling, ambiguous
receipt no-replay policy, generation health fencing, final drain and cancellation.
No new response aggregation, logging, metrics, durable-delivery guarantee or live
configuration cache is part of this proposal.

Original constraints remain in the security NDJSON escaping, concurrency bulk-body
cap and LinkSys date-pattern admission review pages. Existing scripted retry and
partition regressions are useful controls, not proof of this unimplemented
ownership change. No runtime implementation or allocation-lifetime probe was added.
