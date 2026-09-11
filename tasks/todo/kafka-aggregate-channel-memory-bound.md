# Bound Aggregate Kafka Topic-channel Memory

## Meta
- Status: pending
- Severity: medium
- Category: resource-management

## Evidence
Each newly observed topic creates an independent queue with `channel_size` owned `String` entries. The client bounds each queue but does not bound topic count, aggregate queued records, or aggregate payload bytes.

## Why separate
A safe global bound needs an operator-facing policy and must define admission, eviction, error reporting, topic cardinality, and byte accounting. Those choices cross schema, sender, runtime, and observability ownership.

## Impact and open questions
- Many topics or large queued payloads can multiply memory beyond the apparent per-channel limit.
- Record-count limits do not bound bytes.
- Failure policy for new topics versus existing queues is undefined.

## Candidate next step
Define bounded aggregate record, byte, and topic limits before changing the runtime. Coordinate schema validation and sender behavior, with focused unit tests and only the integration changes required by the final design.
