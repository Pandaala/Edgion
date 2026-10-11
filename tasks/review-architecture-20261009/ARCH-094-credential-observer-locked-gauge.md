---
id: ARCH-094
status: candidate
kind: behavior-defect
priority: P2
partition: PART-433
analyst: /root/analyze_outbound
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Keep CredentialManager death classification and its Gauge write under one service lock

## Current ownership and concrete failure

`edgion-controller/src/services/credential_manager/service.rs::spawn_worker` registers a process-global `ServiceHandle` under `SERVICE`, publishes the existing `edgion_controller_leader_job_alive{leader_job="credential_manager"}` Gauge as 1, and delegates completion observation to `services::supervision::observe_worker`.

Its completion callback at service.rs:99–105 computes `current` using `SERVICE.lock().as_ref().map(...) == Some(generation)`. That unnamed lock guard is destroyed at the end of the `let current = ...;` statement. The following conditional Gauge-0 emission therefore runs outside the service lock. The generation check and its observable effect are two independently scheduled operations.

A concrete permitted interleaving uses the real non-Kubernetes reload lifecycle: an old worker has ended unexpectedly; its observer checks that generation A is current and is preempted after releasing the lock. `non_kube_common.rs:279,303` stops the service during reload and later starts replacement generation B. Generation B registers its handle and sets the same Gauge to 1. Observer A resumes, writes 0 and returns true, so the canonical observer can also report the old death after replacement. The replacement can be alive while the Gauge remains 0; its ordinary loop does not periodically restore this value. Kubernetes leader-job stop/start uses the same service entry points at center.rs:1386–1399.

This witness requires an already-completed unexpected worker end and an overlapping lifecycle change. `observe_worker` explicitly skips cancelled JoinErrors, so an ordinary abort alone does not invoke this callback. No claim is made that every reload or leadership transition races, or that such an incident has occurred in deployment.

## Existing mechanism and bounded correction

The canonical observer contract in `services/supervision.rs` explicitly requires the captured-generation check and Gauge-0 write inside the same service-global critical section. Its purpose is to prevent exactly this stop/start interleaving. The DNS resolver callback already retains a named service guard across both operations.

Retain a named `SERVICE` guard while checking the captured generation and conditionally setting the existing Gauge to 0. Return the classification obtained under that guard. If the registered generation differs or is absent, return false and emit no Gauge update. This closes the check/write gap without changing service execution, worker cancellation, restart policy or credential publication.

An atomic load of the generation followed by an unlocked Gauge write still has the same gap. Unconditional zero on completion or stop violates the existing stale-observer and absent-service policies. A new timer, metrics registry, lifecycle framework or automatic worker restart is unnecessary; the existing mutex and callback already own the required boundary.

## Preserved contracts and distinguishing verification

Preserve the existing metric name, fixed label, series-admission policy, current-generation unexpected-death reporting and cancelled-worker suppression. Preserve no-op absent stops and explicit operator/lifecycle recovery after unexpected death. Add no metric, label, log or request-path work.

Keep CredentialManager's independent contracts unchanged: one active identity-keyed memory entry, generated-token expiry, source/consumer UID and frozen-reference checks, exact Lease fencing, Kubernetes Secret CAS/ownership and UID/RV delete preconditions, bounded acquisition concurrency, retry classification, publication-mode transitions, desired-state status projection and serial workqueue requeue. No stronger synchronous-abort, task-drain, credential-erasure or external-publication guarantee is inferred from this metrics correction.

The existing supervision tests exercise panic/current callbacks, clean-return classification and cancellation suppression. CredentialManager tests exercise generation rejection and real memory-publication/currentness seams, but do not cover this callback's atomic generation-and-Gauge transition. Future focused coverage should drive the production classification helper with isolated recording state: an old callback arriving after replacement must leave B's live Gauge intact; a current unexpected end must write zero; absence must create no series. A synchronized interleaving must show that service replacement cannot enter between checking A and its zero-write. Preserve observer wiring coverage and avoid sleeps or a generic service framework solely for tests.

## Original authority, deduplication and limits

The original lifecycle-supervision decision rejects ungated Gauge writes and states that generation comparison and zero publication share the service lock. That authority and the current canonical observer documentation agree with the existing DNS implementation. CredentialManager's callback violates the same bounded contract.

ARCH081 concerns observing the etcd leader task's distinct diagnostic lifetime; ARCH088 concerns unbalanced active-controller scope accounting; ARCH078 concerns Gateway exporter idle eviction. This cause is the unlocked conditional emission in one service's already-installed observer. Controller idle eviction and broader task cleanup are not its remedy.

Classification: behavior-defect, P2. Static source and existing-test inspection only; no edits, builds, tests, application execution, network or Git operations. No measured frequency, production incident or broader supervision guarantee is claimed.
