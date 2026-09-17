---
name: kafka-producer-generation-shutdown
description: Use when reviewing Kafka LinkSys producer task ownership during runtime replacement or deletion.
knowledge_type: regression-guards
finding_id: ISSUE-003
---

# Kafka producer tasks terminate before generation shutdown completes

## Conclusion and scope

Each `KafkaLinkClient` generation owns the producer task for every active topic.
Retirement closes new producer registration, and the runner's shutdown call
aborts and joins the registered tasks before it returns. The persistent
runner's delete and handover cleanup fence therefore includes producer
termination.
This concerns runtime replacement and deletion, not process-exit log delivery.

## Rationale and evidence

`ISSUE-003` identified a detached `tokio::spawn` in
`edgion-gateway/src/link_sys/providers/kafka/client.rs`: a task could retain its
captured rskafka client while waiting for partition initialization or a produce
operation after the old runner completed cleanup. Clearing `topics` only drops
senders; it does not cancel a task already awaiting network work. The 30-second
produce timeout does not cover partition initialization and is not a retirement
bound.

`TopicEntry.producer_handle` now keeps each spawned task with its topic entry.
`KafkaLinkClient.producer_admission` serializes new-topic task registration with
`mark_retired`, so shutdown can enumerate the complete set of active entries,
abort their tasks, and await termination. Self-healing only removes a closed
topic entry once its producer handle reports completion. The runner invokes provider shutdown
before its delete join or retiring-generation join completes. The Kafka lifecycle
design in `tasks/todo/linksys-runtime-apply-lifecycle/04-kafka.md` and D20 in
`10-decisions.md` require this cancellation without draining queued records.

## Alternatives and limits

Dropping topic senders alone cannot interrupt partition initialization or an
in-flight produce. Waiting for the ordinary produce timeout would exceed the
handover retirement bound and would not cover partition initialization. Kafka
still provides fire-and-forget log delivery: cancellation can lose queued or
in-flight records. Existing-topic sends do not take the new registration lock;
the lock only protects creation of a producer task. The separate admission
finding `ISSUE-007` remains an open task and must be revalidated against this
change before its own disposition. The runner gives each generation one
shutdown owner; a separate concurrent call to the public `shutdown` method can
return early through its existing idempotence guard and is outside this fence.

## Verification and re-evaluation

A focused unit test parks a producer task indefinitely, then checks that
`shutdown` terminates it before returning and releases the topic slot. Recheck
this rule if producer tasks move out of `TopicEntry`, topic eviction changes, or
the runner's cleanup fence no longer awaits provider shutdown.

`cargo test -p edgion-gateway --lib shutdown_waits_for_pending_producer_tasks_to_terminate`
passed (1 test). This test exercises the shutdown fence with a pending task;
the producer-registration race is checked from the shared admission lock and
retirement call chain, without a live Kafka broker.

## Resolution references

- `ISSUE-003` — fixed by retaining, aborting, and joining Kafka producer tasks
  within their client generation.
