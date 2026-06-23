---
name: kafka-health-flag-relaxed-ordering
description: Use when reviewing Kafka/LinkSys health-flag findings; covers why `Ordering::Relaxed` on the advisory `healthy` AtomicBool is correct and must not be "upgraded".
status: accepted-tradeoff
finding_id: link-sys-kafka-health-01
closed: 2026-06-16
---

# Kafka `healthy` flag uses `Ordering::Relaxed` — NOT-AN-ISSUE

## Conclusion

The `record_produce_result` fold and every read/write of `KafkaLinkClient.healthy: Arc<AtomicBool>`
(`edgion-gateway/src/link_sys/providers/kafka/client.rs`) intentionally use `Ordering::Relaxed`.
This is correct. Suggestions to "upgrade" the flag (or the `last_flip_log_ms` rate-limit timestamp)
to `Acquire`/`Release`/`SeqCst` for safety are **not accepted**.

## Core Rationale

**1. The flag is a standalone advisory hint, not a synchronization gate.**

`healthy` does not guard publication of any other shared data. The data path is independently
synchronized: `try_produce` does its own `inner_client.load_full()` (`ArcSwapOption` carries its own
ordering) and bails on `None`. `AccessLogger::send()` reads `healthy()` only to pick the first
healthy sender — a routing hint. A stale-by-nanoseconds read at most mis-routes one access-log
entry, which the fire-and-forget contract (I5) already tolerates (the entry may be dropped). There
is no `healthy == true ⇒ inner_client visibly Some` invariant for an acquire/release pair to protect.

**2. `compare_exchange(Relaxed, Relaxed)` already gives single-winner semantics.**

The multi-topic tasks share one flag; CAS guarantees exactly one task observes the transition and
acts (emits metric + rate-limited log). Losers see the already-correct value and no-op. This is
stronger than the etcd monitor's `load`-then-`store(true)` (which is only safe there because etcd
has a single monitor task). No lost-update or wedged-state failure exists for a single bool.

**3. A spurious stale-CAS flip is self-correcting.**

A late failure CAS can flip an already-recovered flag back to unhealthy; the next successful produce
corrects it. Documented inline; acceptable because the flag is advisory.

## Fix Suggestions Not Accepted

- "Use `Acquire`/`Release` so the gateway sees a consistent client state" — there is no co-published
  state; `inner_client` has its own sync. Adds cost, fixes nothing.
- "Use `SeqCst` to be safe" — pure overhead; no cross-variable ordering requirement exists here.
- "The rate-limit timestamp load/store is a TOCTOU race" — it is a best-effort log throttle, not a
  correctness barrier; a one-off double-log under contention is acceptable and documented.

## Re-evaluation Triggers

Re-open only if:

- A future change makes another reader depend on a happens-before relationship between `healthy`
  and `inner_client` (or any other field) — i.e., the flag stops being purely advisory.
- Task 05's background probe shares the `healthy` flag across a respawn path AND relies on the flag
  to publish freshly-rebuilt client state (then audit the ordering at that new write site).

## Reference Cases

- 2026-06-16 Kafka health-recovery fix (spec `docs/superpowers/specs/2026-06-15-kafka-health-recovery-design.md`).
- Source: `edgion-gateway/src/link_sys/providers/kafka/client.rs` (`record_produce_result`, `flip_log_allowed_at`).
- Consumer: `edgion-gateway/src/observe/access_log/logger.rs` (first-healthy routing).
