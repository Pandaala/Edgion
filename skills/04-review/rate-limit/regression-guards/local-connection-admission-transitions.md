---
name: local-connection-admission-transitions
description: Review local connection token-bucket concurrency, persistent entry binding, and lazy runtime-generation activation.
knowledge_type: regression-guards
---

# Local connection admission and configuration transitions

## Conclusion and scope

D0100 identified lost token decrements and incoherent configuration updates in
the Gateway's local `ConnectionRateLimit` budget. The correction makes each
entry/port admission one locked transition: initialize if necessary, settle
elapsed refill, adopt a newer configuration, adjust capacity, and consume.

The canonical authorities are the atomic shared-state and coherent-publication
rules in [the design index](../../../00-design-rules.md), together with the
existing [entry identity and lifetime rule](connection-rate-limit-entry-scoped-buckets.md).
The user approved per-port first-use activation and its old-runtime behavior
before implementation. This is not a requirement for every future limiter to
use a mutex, nor a cross-process transaction guarantee.

## Rationale and evidence

The previous `local_gate.rs::refill` locked refill only, loaded a balance, and
later stored a replenished value. A concurrent successful consumption CAS could
be overwritten. `update_params` separately wrote rate, interval and balance and
could also overwrite consumption. Every connection acquired the process-global
registry mutex and reapplied the calling runtime's parameters, allowing old and
new runtimes to alternate capacity credits.

`connection_rate_limit/local_gate.rs` now owns:

- `LocalGateRegistry`: the same process-lifetime budget registry. Construction
  binds an entry once; no connection hit looks up a scope in the global map.
- `LocalGateBinding`: immutable entry handle, checked generation and local
  rate/interval for a compiled runtime. Reservation alone never changes a bucket.
- `EntryGates`: one sparse `u16` port map per entry. Hits clone the bucket Arc
  under a read lock and drop it before taking the bucket mutex. Misses recheck
  under a write lock and publish a single unactivated bucket.
- `LocalRateGate.state`: `Mutex<Option<GateState>>`. The first locked admission
  chooses the initial configuration, even if another generation created the slot.
- `GateState`: active generation, parameters, `u128` milli-token balance and
  monotonic refill timestamp. Zero is a valid initialized timestamp. The local
  arithmetic proof and measured cost are in the
  [algorithm contract](../../../12-algorithms/local-connection-token-bucket.md).

`ConnectionRateLimitPlugin::new` reserves only for valid local configurations;
reservation exhaustion uses existing invalid-init denial. `on_connection` consumes
through the bound entry and releases all gate locks before metrics, denial-string
construction, or Redis calls. Existing policy Block/Bypass/disabled handling and
the runtime's front fail-closed gate remain authoritative.

### Generation boundary

Production construction order follows the per-kind `CacheData` writer lock across
`full_set`/`partial_update` compilation and store publication. It is not a contract
for arbitrary parallel direct calls that publish constructed runtimes in reverse.

A port adopts a higher generation only when that generation reaches admission.
It first refills using its old parameters, then adds only a capacity increase or
clamps on a decrease. A changed interval starts at the effective current sample;
unchanged parameters retain any uncredited elapsed fraction. Denial still commits
a valid generation change. Older invocations cannot restore older parameters.

Ports advance independently; unused intermediate generations do not create budget
events. Unpublished or front-gate-blocked runtime construction is inert. Removing
or disabling an entry does not evict it; a resumed identical identity retains its
budget and accrues idle refill at the last activated rate. Generation reservation
never wraps and does not use the refill timestamp as a generation marker.

## Alternatives and limits

- Pinned old invocations may consume newer activated local parameters while their
  Redis checks retain old runtime settings. Local and Redis admission are sequential
  budgets, not an atomic policy migration. No local refund follows Redis denial.
- The [layered-attachment rule](connection-rate-limit-layered-attachments.md)
  still applies, including two consumptions for the identical listener/TCP reference.
  `DirectPeerIp`, Redis order, timeout and failure-policy defaults are unchanged.
- The [D0099 scheduling tradeoff](../../listeners/intentional-design/bounded-serial-connection-admission.md)
  remains separate; this fix does not isolate Redis waits from Pingora accept tasks.
- The local `u128` representation avoids overflowing scaled balance/refill. Its
  construction boundary also enforces the shared configuration/Lua/Redis numeric
  domain owned by `edgion-resources/src/redis_rate_limit.rs`.
- This is the existing no-eviction registry's representation replacement, not a
  new parallel store or third generic cache. Entry count can grow with operator
  churn; a `u16` port bound does not bound total process memory.
- Per-entry read-lock contention remains possible. The algorithm page records
  regressions in several single-worker microbenchmark cases; neither the
  correction nor the independent review claims uniform performance improvement.

## Verification and re-evaluation

Verified on 2026-09-10:

- `cargo test -p edgion-gateway --lib plugins::stream --locked`: 83 passed,
  one measurement test ignored.
- `cargo test -p edgion-gateway --lib rate_limit --locked`: 91 passed,
  one measurement test ignored. These filters overlap; counts are not additive.
- The isolated optimized local-state harness using `parking_lot 0.12.5` passed
  20 tests. The explicitly run contention fixture is recorded in the algorithm page.
- `cargo fmt --all`, workspace all-target check and Clippy passed. Clippy emitted
  existing repository warnings, including the unchanged plugin interval clamp.
- Agent-document, SSA-force, metrics-inventory, Pingora-source and Gateway tracing
  guards passed. SHA-256 verification found all 3,361 frozen integration files unchanged.
- Independent full code/knowledge review and a follow-up review of the final
  `parking_lot` correction returned LGTM. No full workspace test suite or live
  cross-process integration run was performed for this correction.

Tests exercise fixed-time exact capacity, controlled refill/consumption overlap,
concurrent first use, once-only expansion, shrink/interval races, stale/skipped/
equal generations, zero/reversed samples, full local integer representation,
generation exhaustion, isolation and retained identity. Runtime/store/bridge tests
execute admissions across full/delta updates, delete/recreate, disabled/invalid/
policy states and layered attachments, rather than only checking pointer identity.

Re-evaluate if construction/publication stops being serialized, activation must
occur at publication rather than first use, buckets gain eviction, numeric or
rounding semantics change, or measured port-map contention matters to the target
deployment. Deterministic Gateway tests do not replace a cross-process hot-reload
traffic test; existing integration files remain frozen for this task.

## Resolution references

- `D0100` / `make-local-rate-gate-transitions-linearizable`: confirmed local
  correctness defect; user-approved design implemented and verified.
- `gw-plugin-acl-rl-04-stream`: earlier identity correction, not evidence that
  concurrent admission or generation changes were already correct.
