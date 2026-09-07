---
name: etcd-leadership-gate-not-fenced-at-txn
description: Use when reviewing findings that the etcd conf-center leadership gate is TOCTOU / a stale leader can commit one write after losing its lease / proposing to add the election key's create_revision as a second compare; documents why the residual ~2-RTT window is an accepted tradeoff scoped to the HA work.
status: accepted-tradeoff
finding_id: etcd-02
closed: 2026-07-17
knowledge_type: intentional-design
---

# etcd Leadership Gate Cannot Fence a Stale Leader at the Transaction Boundary — ACCEPTED-RISK

## Conclusion

The `LeadershipGate` check that opens every mutating `EtcdStorage` method is a
process-local check-then-act; the etcd transaction that follows carries no
leadership condition, so a leader that loses its lease mid-write can still commit
one config write. This is a **pre-existing** boundary — not a regression — and
closing it is scoped to the HA work, not to a narrow review fix. It was **not**
documented in-repo before this file; see the sourcing caveat below before citing
any spec as having blessed it.

Fix suggestions of the form "thread the election key's `create_revision` into
`EtcdStorage` and add it as a second `Compare` in `txn_put_if_mod_revision`" are
**correct in direction but not accepted as a review-issue fix** — see the cost
breakdown below.

## Core Rationale

**1. It is pre-existing, and the same shape has been reasoned about before**

The gate is pre-existing, not introduced by the Center resource-UI CAS work:
`git diff` shows the `ensure_leader()` calls as unchanged context and
`leadership_gate.rs` untouched. Do not open this as a bug against that change.

**Sourcing caveat — read before citing this.** The etcd write gate's own
rationale is **not** documented in-repo. `storage.rs:41-43`,
`leadership_gate.rs:1-2` and `leadership_gate.rs:72` cite "spec §4.6" /
"spec §4.6, §6.2", but no such section exists anywhere under `docs/` — that spec
was never committed. The
TOCTOU self-assessment in
`docs/superpowers/specs/2026-07-13-controller-leader-job-consolidation-design.md:155-160`
— *"This check is TOCTOU — leadership can drop immediately after it returns
`true`. It is not a proof of safety and is not presented as one. It narrows the
exposure from a 30-second-plus window to a few instructions…"* — is about a
**different subsystem**: a *proposed* `leader.is_leader()` gate on
`spawn_caches_ready_task` in the *Kubernetes* center (`center.rs:498`,
`component = "kubernetes_center"`). That proposal never landed either —
`start_leader_jobs` has zero hits in the repo — so it is a record of how the
project *reasoned* about a TOCTOU leader check, not shipped behavior and not a
statement about the etcd write path. **This file is the first in-repo record of
the etcd-path tradeoff.**

The gate is nonetheless more careful than a bare `AtomicBool`: `check()`
(`leadership_gate.rs:130-137`) requires both `is_leader()` and `lease_is_fresh()`,
and freshness (`:74-85`) is `mono_elapsed.max(wall_elapsed) < lease_duration` — a
real `max(monotonic, wall)`, so a VM suspend cannot make a stale lease look fresh.
The `last_mono == 0 || last_wall == 0` sentinel fails closed.

**2. The exposure is ~2 RTT and the CAS already narrows it**

The residual sequence is: gate passes at t0 with a nearly-expired lease → the
node is partitioned and the lease expires → the already-in-flight `get()` +
`txn_put_if_mod_revision` commits at ~t0 + 2·RTT. Before the Center resource-UI
CAS work, `update_one` was a blind `txn_put_if_exists` and a stale leader
clobbered whatever the new leader had written. Now a stale write **fails**
whenever the new leader has already touched the same key; the residual is only
the case where the new leader has not yet touched that key.

**3. The real fix is cross-module and needs verification tooling that does not exist**

`LeadershipGate` exposes only a boolean `check()`. Carrying the election key +
`create_revision` means touching `leader_election.rs`, `leadership_gate.rs`,
`center.rs` (both `new_gated` call sites), `storage.rs` and `status.rs`.
`ConfWriterError` additionally needs a distinct "lost leadership mid-write"
mapping that callers must not confuse with the operator-facing `Conflict` (409)
meaning "your `If-Match` was stale" — a leadership loss is not the client's fault
and must not tell the operator to retry with a fresh copy. This is a real
boundary, not a hypothetical one: a failed compare returns `false` →
`cas_conflict(..)` → `ConfWriterError::Conflict` → 409 (`api/common.rs:81-88`),
and `failed_mod_revision_compare_is_a_retryable_conflict` (`storage.rs:391`) pins
that mapping — an election-key compare in the same `Txn` would be
indistinguishable from a stale `If-Match` at that boundary.

Verification needs a real etcd with an *induced partition*. The real-etcd harness
itself does exist (`#[ignore]`d tests in `etcd/leader_election.rs`,
`link_sys_core/providers/etcd/{election,ops}.rs`, and one in `etcd/storage.rs:376`),
so the gap is partition tooling plus lease-expiry injection between `get()` and
the transaction — not the harness. `etcd/storage.rs`'s own tests are otherwise
pure-helper unit tests.

## The Direction When It Is Picked Up

etcd's native idiom is to make the election key part of the same transaction so
the store — not the client — arbitrates:

```rust
Txn::new()
    .when(vec![
        Compare::create_revision(election_key, CompareOp::Equal, leader_create_revision),
        Compare::mod_revision(full_key, CompareOp::Equal, expected_revision),
    ])
    .and_then(vec![TxnOp::put(full_key, value, None)])
```

Once the lease expires, etcd deletes the election key, its `create_revision` no
longer matches, and every write from the former leader fails atomically —
regardless of whether the new leader has touched the data key. This is what
etcd's `concurrency.Election` does internally. `write_native_status`'s blind
`put_string` (`etcd/status.rs:66-75` — `ensure_leader()` then a plain
`put_string`, no CAS at all) needs the compare too, or it stays a hole.

## Fix Suggestions Not Accepted

- Opening this as a bug against the Center resource-UI CAS change — that change
  strictly improved the window; it did not introduce it.
- Patching only `update_one_if_version` with the election compare — leaves
  `set_one`, `create_one`, `delete_one_if_version` and `write_native_status` as
  holes, and splits the error-mapping question across two changes.
- Tightening `lease_is_fresh()`'s margin instead — shrinks the window but cannot
  close it; the gap is structural (the store never sees the condition), not a
  matter of clock slack.
- Mapping a mid-write leadership loss onto the existing `Conflict` (409) —
  misattributes a server-side failover to the client's `If-Match`.

## Re-evaluation Triggers

Re-open this decision only if:

- A real split-brain write is observed in an incident, rather than reasoned about.
- The HA work lands and this becomes a scoped deliverable with a real-etcd
  partition test harness to verify it.
- `LeadershipGate` grows an election-key/revision-carrying API for another
  reason, which would make the second `Compare` a cheap add-on.

## Test Gap (recorded, not closed)

- An etcd integration test that expires the leader's lease between `get()` and
  the transaction and asserts the write is rejected. Cannot be written against
  the current pure-helper tests.
- A test that a mid-write leadership loss surfaces as a distinct error from a
  stale-`If-Match` 409.

## Reference Cases

- center-resource-ui-contract review (2026-07-16), finding P2 — closed 2026-07-17.
  The original issue file cited the spec below as the canonical statement of *this*
  tradeoff; that attribution is wrong (see the sourcing caveat above) and was
  corrected on closure.
- `docs/superpowers/specs/2026-07-13-controller-leader-job-consolidation-design.md:155-160`
  — precedent only, and from an unlanded proposal: how the project reasoned about
  the *Kubernetes* center's analogous TOCTOU leader check. Not about the etcd
  write path, and not shipped behavior.
- `edgion-controller/src/conf_mgr/conf_center/etcd/storage.rs` (`ensure_leader`),
  `etcd/leadership_gate.rs`, `etcd/status.rs`,
  `edgion-common/src/link_sys_core/providers/etcd/ops.rs` (`txn_put_if_mod_revision`).
- Sibling leader-election decisions: [leader-handle-polling.md](leader-handle-polling.md),
  [leader-lease-release-on-shutdown.md](../regression-guards/leader-lease-release-on-shutdown.md).

## Resolution references

- `etcd-02` — 2026-07-17 (⚠️ accepted tradeoff).
