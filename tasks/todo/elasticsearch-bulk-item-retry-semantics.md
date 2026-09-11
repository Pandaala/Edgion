# Define Elasticsearch Bulk Item-level Retry Semantics

## Meta

- Status: pending
- Severity: medium
- Category: reliability

## Evidence

An Elasticsearch bulk request can return HTTP success with `errors: true` while individual items fail transiently, including `429 es_rejected_execution_exception`. The current provider counts and warns about those failures, then treats the batch request as complete, so failed items are dropped without retry.

## Why separate

An isolated retry branch would not define safe delivery semantics. Correct handling spans response-item/input correlation, transient versus permanent classification, bounded body reconstruction and retry accounting, shutdown draining, and the duplicate-delivery implications of retrying only part of a bulk request.

## Impact and open questions

- Transient item failures can lose access-log records even though the bulk HTTP request succeeded.
- Retry classification and maximum work must stay bounded under Elasticsearch overload.
- The contract must define whether partial retries preserve ordering and what duplicate delivery is acceptable.
- Shutdown behavior must not silently bypass or unbound item-level retries.

## Candidate next step

Design and test a bounded item-level delivery contract before changing the bulk loop. Cover response correlation, retryable status/error types, partial request reconstruction, retry limits/backoff, duplicate semantics, and shutdown deadlines. Add or update integration coverage only when the implementation requires it.
