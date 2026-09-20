---
name: kafka-health-flag-relaxed-ordering
description: Use when reviewing Kafka/LinkSys health-level ordering; covers why lock-free relaxed reads remain correct inside the publication-aware health cell.
status: accepted-tradeoff
finding_id: link-sys-kafka-health-01
closed: 2026-06-16
knowledge_type: intentional-design
---

# LinkSys health level keeps relaxed atomic reads — NOT-AN-ISSUE

## Conclusion

`KafkaLinkClient` now uses the shared `PublishedHealth` cell. Its request-side
level read and the cell's internal atomic level access intentionally use
`Ordering::Relaxed`. Writers serialize level changes and publication ownership
with the cell's mutex, while the persistent runner transfers that ownership
around the current-client ArcSwap. Upgrading the atomic level or the
`last_flip_log_ms` rate-limit timestamp to Acquire/Release/SeqCst is not useful
without a new cross-field synchronization requirement.

## Core rationale

**1. The level is an advisory hint, not a synchronization gate.**

`healthy` does not guard publication of any other shared data. The data path is
independently synchronized: `try_produce` loads `inner_client` through its own
ArcSwap and fails on `None`. `AccessLogger::send()` reads `healthy()` only to
select the first healthy sender. There is no `healthy == true` implication that
requires another field to become visible through an acquire/release pair.

**2. The transition mutex gives single-winner semantics.**

Kafka's topic tasks share one cell. `PublishedHealth::update` holds its short
transition mutex while comparing and storing the level, so exactly one writer
observes a change and invokes the published-generation reporter. Redis, Etcd,
Elasticsearch, and OTLP use the same duplicate suppression. Atomic ordering does
not establish that lifecycle ownership; the mutex and runner publication do.

**3. Concurrent external observations remain advisory.**

A late failure observation can follow a recovery observation and become the
latest level; the next successful operation or probe corrects it. The transition
is real in observation order and is counted only if that generation is published.
No atomic ordering mode can infer the chronology of external results.

## Fix suggestions not accepted

- "Use Acquire/Release so the Gateway sees a consistent client state" — there is
  no co-published state; `inner_client` has independent synchronization.
- "Use SeqCst to be safe" — it adds ordering without defining a cross-variable
  invariant.
- "The rate-limit timestamp load/store is a TOCTOU race" — it is a best-effort
  log throttle; a one-off double log is accepted and does not affect metrics.

## Re-evaluation triggers

Re-open if a reader uses the health level to synchronize access to another field.
That change must first define the new invariant and then select the required
ordering. Also re-evaluate if health reads cease to be a request-path routing hint.

## Reference cases

- 2026-06-16 Kafka health-recovery fix (spec
  `docs/superpowers/specs/2026-06-15-kafka-health-recovery-design.md`).
- Source: `edgion-common/src/link_sys_core/health.rs` (`PublishedHealth`) and
  `edgion-gateway/src/link_sys/providers/kafka/client.rs`.
- D0007 publication-aware transition unification superseded the Kafka-local CAS
  fold while retaining relaxed advisory reads.
- Consumer: `edgion-gateway/src/observe/access_log/logger.rs` (first-healthy routing).
