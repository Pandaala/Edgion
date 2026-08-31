# D0008 — Isolate Kafka Producer Tasks by Runtime Generation

## Meta
- Status: needs-investigation
- Severity: medium
- Discovered in: B0048, `edgion-gateway/src/link_sys/providers/kafka/client.rs`
- Category: concurrency

## Evidence
Kafka shutdown clears the sender map but retains no producer task handle or generation token. Existing receivers may continue draining queued records after replacement or removal, and an old task can still update health for the same resource key.

## Why deferred
Correcting this spans task ownership, replacement, queue draining, cancellation, health publication, and delivery guarantees. A local abort or health guard would choose lifecycle semantics without the required concurrency design and tests.

## Impact and open questions
- Old generations may deliver records after replacement or removal.
- Health updates from old and new generations can race on the same resource label.
- The contract must define graceful draining versus prompt cancellation and bounded shutdown.

## Candidate next step
Design generation-owned producer tasks with explicit cancellation, bounded drain behavior, and generation-checked health publication. Add deterministic unit-level concurrency coverage without modifying existing integration-test files during this campaign.
