---
id: ARCH-027
status: candidate
kind: architecture-debt
priority: P2
primary_partition: PART-273
independent_verification: confirmed
---

# Bound Elasticsearch bulk acknowledgement reception and projection

## Actual exchange and missing owner

Elasticsearch bulk.rs:314 send_bulk_request enforces outgoing sub-batch bounds
but reads successful acknowledgements using resp.json::<BulkResponse>().await
at line 342, without an independent receiving byte ceiling. BulkResponse retains
_index, _id, took and arbitrary nested error: serde_json::Value, although retry
classification principally uses ordered index-result presence and status.

Production access logging reaches EsLogSender, current EsLinkClient and the bulk
worker; backfill uses the same worker through awaiting submission. An oversized
acknowledgement or large item-error envelope can be accumulated and deserialized
independently of the bounded outgoing document body. Timeout and submitted item
count do not bound receiving bytes or the retained arbitrary error tree. This
is source-derived debt; no RSS, throughput, incident or reproduction is claimed.

## Bounded improvement and alternative

Define a private acknowledgement ceiling enforced while receiving, then parse
a minimal projection needed by the classifier. Choose it deliberately for
supported batch cardinality; the plugin-call 1 MiB default is not automatically
the ES contract. The capped reqwest reader at plugins/http/common/outbound/call.rs
provides existing mechanics. If reuse requires extraction, move only a small
shared receiving primitive, keeping provider classification here.

Content-Length checking alone misses chunked/misdeclared responses; projection
alone leaves the full input read unbounded. Keep both a receiving cap and the
minimal retained shape. This bounds one exchange's receiving state; it is not a
process-wide memory ledger or permission for main response aggregation.

## Invariants, migration and future verification

Oversized/malformed responses retain the existing ambiguous-outcome policy:
never retry the entire batch blindly and duplicate acknowledged documents.
Keep exact action/count/order correlation, 429-only subset retries, one shared
retry budget, cancellation, generation health, queue/backfill and shutdown
semantics. Dropping unused fields must not weaken classifier checks. Request
BodyStorage and the main proxy response remain outside this exchange.

Original max-body, item-retry, shutdown, healthy/fallback/backfill and exit
decisions were inspected; they accept delivery ambiguity and queue marking, not
an acknowledgement memory bound. Migration seams are the worker receiving helper
and response projection. Future checks cover declared/chunked oversize, exact
boundary success, huge ignored errors, action/count mismatch, subset-only retry
and cancellation. Static only; source/knowledge receipts are in the partition
report and independent verification. No test or build was run.
