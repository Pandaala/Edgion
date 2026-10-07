---
name: kafka-aggregate-queue-bounds
description: Use when reviewing Kafka LinkSys topic creation, producer queues, or pending-record memory accounting.
knowledge_type: regression-guards
finding_id: kafka-aggregate-channel-memory-bound
---

# Kafka producer state has per-client aggregate bounds

## Conclusion and scope

Each `KafkaLinkClient` applies three aggregate limits in addition to its
per-topic `channelSize`: `maxTopics`, `maxPendingRecords`, and
`maxPendingBytes`. A record must reserve both pending-record and UTF-8 payload
byte capacity before it enters a topic queue. The reservation remains held
while the record is queued and while `rskafka` is producing it.

Reaching any limit rejects the new non-blocking submission. Queue saturation
does not change the Kafka health flag; it remains backpressure rather than a
transport failure.

## Rationale and evidence

The Gateway bounded-memory rule in `skills/00-design-rules.md` applies to this
data-plane log path. A per-topic channel bound alone did not bound the number of
topic queues, their aggregate record count, or the size of their owned strings.

`KafkaLinkClient::try_produce` now obtains a `QueueReservation` and a topic slot
before enqueueing. The reservation uses shared atomic counters and travels in
`QueuedRecord`. Its `Drop` releases both counters after produce completion, on
an enqueue error, or when a receiver and its queued records are dropped.
`TopicReservation` similarly releases the topic slot when the map entry is
removed or the client shuts down. Controller-side `LinkSys::validate_config`
rejects zero limits, and `KafkaLinkClient::from_link_sys` derives the actual owning
Kafka config and repeats the check at the runtime construction boundary.

## Alternatives and limits

The byte limit counts `String::len()` for the UTF-8 payload. It is a logical
payload budget, not a measurement of allocator, channel-node, task, Kafka SDK,
or protocol framing overhead. Topic queues remain resident until their producer
closes or the client shuts down; `maxTopics` bounds that retained state without
adding idle eviction to the hot path.

Producer tasks from a replaced client generation are a separate lifecycle
concern tracked by `tasks/todo/linksys-runtime-apply-lifecycle/`. Each such
client generation owns an independent bounded budget.

## Verification and re-evaluation

Focused resource tests cover zero-value admission. Focused Gateway tests cover
default resolution, exact record and byte limits, rollback after a failed
second reservation, release on drop, and topic-slot reuse. Re-evaluate the
accounting if the queue payload stops being an owned `String`, Kafka production
becomes multi-record per reservation, topic entries gain idle eviction, or the
producer task lifecycle changes.

## Resolution references

- `kafka-aggregate-channel-memory-bound` — fixed by adding per-client topic,
  pending-record, and pending-byte budgets to the Kafka LinkSys producer.
