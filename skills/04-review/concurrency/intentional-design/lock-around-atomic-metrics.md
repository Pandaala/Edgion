---
name: lock-around-atomic-metrics
description: Use when reviewing findings about holding `RwLock` / `Mutex` write locks across `metrics` crate gauge/counter calls; gives the hot-path-vs-control-plane decision rule and the snapshot-then-emit refactor when warranted.
knowledge_type: intentional-design
---

# Holding a Write Lock Across `metrics` Calls — Style Concern, Not a Latency Bug

## Current backend boundary (ARCH-043)

Fixed class handles update exporter atomics. Dynamic proxies take short per-slot
operation gates; gauges with overlapping owners must publish committed deltas
inside the subsystem mutation lock. Do not move this publication outside its
owning lock or use stale absolute snapshots based on the historical claim below.
Scrape serialization and dense sketch destruction occur after slot gates release;
request operations never wait for full rendering. Re-evaluate real lock ordering
and measured current latency when a dynamic metric is involved.

## Historical atomic-exporter assessment

**Finding pattern**: a function holds a `parking_lot::RwLock` (or `Mutex`) write lock across a call that emits `gauge!(...).set(...)` / `counter!(...).increment(...)` / similar `metrics`-crate operations. Reviewer flags it as "lock held during I/O / lock contention risk" and proposes splitting into snapshot-inside-lock + emit-outside-lock.

**Verdict**: **the latency claim is wrong on `metrics` 0.24** — Prometheus / OTel exporters back gauges and counters with atomic stores; there is no I/O and no allocation on the hot edit. So today the pattern is harmless.

But the **structural concern is legitimate**: a future metrics backend swap (push-based exporter, custom registry with locking) could re-introduce real cost under the lock. For control-plane code with low call rates, splitting the function is cheap insurance.

**Decision rule**:

- **Hot path** (per-request, per-connection): keep the metric call inline; do not introduce a snapshot indirection just for this. The cost of an extra `HashMap` allocation per request dwarfs any hypothetical future backend cost. Same logic as the `record_status_write` entry.
- **Control plane / event-driven** (register, disconnect, reconcile, admin API): if the lock holds the whole map and the metric call iterates the map, prefer the split (`compute_*_snapshot` inside lock → `emit_*` outside). The cost is one local `HashMap` per event; the events fire ~minutes apart.
- **Single-call, single-label** under lock (e.g. `gauge!("foo").set(1.0)` in a control-plane setter): not worth touching — one atomic store inside a write lock that's already held for actual mutation.

**When to re-report**: only if a *new* control-plane path holds a `RwLock`/`Mutex` write lock around iteration-and-emit AND the call rate could plausibly exceed 100/sec. Single-shot gauges or sub-iteration emits are accepted as-is.
