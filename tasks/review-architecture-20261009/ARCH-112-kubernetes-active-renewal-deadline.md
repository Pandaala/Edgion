---
id: ARCH-112
status: candidate
kind: behavior-defect
priority: P2
partition: PART-400
analyst: /root/verify_ip_union
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Keep the active renewal deadline effective during election IO

## Current ownership and evidence

LeaderElectionConfig validation requires retry_period_secs < renew_deadline_secs < lease_duration_secs; defaults are2/10/15 seconds. Schema, feature reference and source describe the renewal window since the last acknowledged renewal during which transient errors may retain leadership.

LeaderElection::run at leader_election.rs:243–447 owns last-successful-renewal Instant and authoritative leader state. It awaits retry sleep, optionally awaits an unsynced Pod-label retry, then awaits try_acquire_or_renew. Deadline comparison occurs only after a non-404 error returns. Pending Lease GET/replace and Pod-label IO are not selected against the deadline. After acknowledged acquisition, the loop publishes leadership before awaiting its Pod patch; subsequent failed-label retries precede Lease renewal. Derived routing state shares the sequential path that must renew or withdraw authority.

The actual center uses Client::try_default at center.rs:1461 and separately spawns election at1477–1488. Pinned kube-client2.0.1 default connect/read/write timeouts are30/295/295 seconds, applied by its transport builder; those are not the ten-second renewal deadline or a claimed precise RPC duration bound. Center leadership watchers and ordinary status-write guards read LeaderHandle::is_leader rather than independently checking the loop's private renewal Instant. Typed live-Lease fencing is a separate defense for its actual callers.

## Concrete failure scenario and structural cause

After an acknowledged renewal, a subsequent Lease request can remain pending beyond renew_deadline. Its later error branch cannot run while pending, so local leadership remains true. A pending acquired-state or retry Pod patch similarly delays renewal/expiry observation. If renewal stays absent beyond lease_duration, another replica may acquire the expired Lease while this replica still advertises its previous flag.

The witness needs delayed/stalled IO, not promptly returning transient errors. It is a source-supported interleaving, not a reproduced incident, exact measurement or universal overlap claim. The expiry owner implements an after-operation classification rather than an active boundary around awaited work; config validation alone cannot enforce it.

## Bounded improvement and alternatives

Keep renewal timing and withdrawal in the existing election owner. Activate the remaining monotonic deadline since the last acknowledged renewal while a held term awaits retry sleep, Lease IO or derived-label IO. On expiry, withdraw flag and clear current fence before further best-effort IO; an expired operation must not reassert that term. Keep the single tick scheduler/label convergence flag, with a bounded opportunity for labels that leaves renewal runnable and retries current state on later ticks. Merely reordering labels after renewal does not bound pending Lease IO or a transition patch.

Use a small private election-local change, not a generic SDK timeout framework, extra retry task or job registry. Do not shorten the shared client's global timeout or require label success before announcing acquired authority. Cancelling a submitted replace/patch does not prove server rejection: keep the expired local term withdrawn and require existing fresh read/CAS acquisition before republishing. No exactly-once write or cross-process fencing guarantee follows.

## Required invariants and future verification

Preserve configured transient-error grace/default cadence, immediate authoritative404 and competing-holder/409 step-down, Lease UID/term, resourceVersion CAS, transitions/acquire-time semantics, preflight label reset/RBAC failure, tick-based convergence and bounded best-effort shutdown release. Preserve leader-only resource-epoch cancellation, all-serve resource-cache retention, separate typed fencing and local-at-write authority. Add no metrics, restart policy, handler IO, business queue, schema field or Gateway request mechanism.

Drive the real run loop using an in-process kube Client: acquire successfully then hold Lease GET or replace; separately hold acquired-state Pod patch and a subsequent unsynced-label retry. Establish active-deadline flag/fence withdrawal without waiting for held IO, and no revival by late completion. Preserve timely renewals, transient grace,404/409 loss and fresh acquisition. Production-owner tests must replace arithmetic-only testing, with no network.

Full actual fence tests classify current/changed-term/missing/unavailable/local-step-down via an in-process service; they do not exercise pending IO in run. Actual center tests cover bootstrap/drain cancellation rather than this await boundary. No tests/builds/applications were executed.

## Original decisions and limits

The original leader-pod-label-tick-resync decision rejects dedicated retries, forced election after patch-failure counts and blocking acquisition announcement on label success. It states that global API failure should cause step-down through renewal_deadline, treating labels as derived state. Those requirements remain. The accepted post-ready progress-stall limitation governs the center startup-only resource-epoch liveness deadline, not this existing election renewal contract. Accepted2–10-second polling/grace does not authorize pending IO to extend it to transport timeouts.

ARCH-081 covers Etcd post-win diagnostics before supervision; this is Kubernetes Lease/label IO under a configured renewal deadline and needs no cross-backend framework. ARCH-088/094 gauge ownership is distinct. Some originals retain stale AtomicBool prose; actual watch sender/consumers establish current ownership. No scheduler-starvation/non-yielding-task, clock-skew fencing, startup probing or general election-panic supervision claim is made. Analyst: /root/verify_ip_union; independent decision pending.
