---
id: ARCH-079
status: candidate
kind: architecture-debt
priority: P3
partition: PART-228
analyst: /root/analyze_conversion
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-079: Disarm Etcd lock fallback cleanup after successful explicit release

## Current ownership and evidence

`edgion-common/src/link_sys_core/providers/etcd/ops.rs:445-478` owns `EtcdLockGuard`, its consuming `unlock` method, and its best-effort `Drop` cleanup. `unlock` awaits the native SDK Unlock RPC, then LeaseRevoke, and returns success. It never marks the guard released. Destruction of the consumed guard therefore enters unconditional `Drop`, clones the SDK client and lock key, and spawns another task that awaits both RPCs again.

This has a current caller: `edgion-gateway/src/api/mod.rs:1342-1374::etcd_lock` acquires a guard through bounded `try_lock` and immediately awaits `guard.unlock()`. Every successful explicit release at this management endpoint schedules a second Unlock and LeaseRevoke attempt. If the spawned task runs and reaches both calls, it sends two additional RPCs after the successful pair; the implementation ignores their outcomes. This is an avoidable cleanup task and second lock-key copy even if the duplicate RPCs fail harmlessly. It is a management path, not evidence of proxy request latency or a production outage.

Cargo.lock pins registry `etcd-client` 0.18.0. Its `src/client.rs:439-440,488-489` delegates each call to native lease/lock clients. `src/rpc/lock.rs:53-59` and `src/rpc/lease.rs:59-66` perform the corresponding RPC and propagate its result; they do not disarm Edgion's guard or coalesce its later cleanup. No server-side duplicate-operation outcome is assumed here.

## Root cause and change scenario

Explicit release and fallback release are two completion paths of one owned resource, but the owner has no completion state. A caller choosing explicit release to observe its result cannot prevent the destructor from repeating already completed work. Maintaining release policy currently requires reasoning about both methods and their implicit interaction rather than one terminal state.

The duplication is unnecessary on the successful path. Fallback cleanup remains legitimate when a guard is dropped without release, an explicit RPC fails, or an in-progress explicit-release future is canceled. Those cases must not be conflated with acknowledged completion.

## Bounded improvement and alternative

Keep native Etcd locking and the existing guard boundary. Record successful completion in the guard only after both awaited explicit operations return success, and have `Drop` return before cloning or spawning when that state is complete. There must be no await between recording completion and returning. Retain an armed guard while either RPC is pending and on either error; dropping the consuming future then retains the current best-effort fallback. Do not move the key out or clear the cleanup state before awaited cleanup succeeds.

A minimal completion bit is sufficient for this finding. An owned optional cleanup state is an alternative if it makes the same error/cancellation invariant clearer, but extracting an executor, adding a client trait, or introducing a generalized lease framework is unnecessary. Tracking successful partial steps could reduce retries further, but it is outside this proposal: the existing fallback may retry both operations after a partial failure. This finding makes no exactly-once network guarantee under cancellation or ambiguous transport results.

## Required invariants and verification

Preserve namespace application, the SDK-returned lock key and lease identity, native TTL expiry, bounded acquisition through `try_lock`, explicit error ordering and management response semantics. Preserve fallback cleanup on ordinary drop, first-RPC failure, second-RPC failure, and cancellation while either RPC is pending. Do not change Controller election/CAS policy, introduce a local SDK, add request-path observability, or broaden transport/security policy.

Read test sources show existing coverage of lease revocation, keepalive and leader-key guards in `edgion-common/tests/unit/link_sys_core/providers/etcd/ops_test.rs`. The real-service integration script `edgion-tests/integration/scripts/integration/link_sys/run_etcd_test.sh:669-682` checks lock acquire/release results through the actual management endpoint; it does not assert cleanup task or RPC counts. Future focused verification should distinguish successful release from error/cancellation and prove completed guards schedule no fallback, while preserving the existing real lock behavior. Prefer a narrow state/ownership check over a production abstraction added for mocks. No tests or service calls were executed in this review.

## Original decisions and limits

The current original `skills/04-review/link-sys/regression-guards/etcd-configuration-and-message-boundaries.md` explicitly retains upstream-native KV/Watch/Lease/Lock semantics after withdrawal of the custom SDK feature set. This proposal changes only Edgion's guard completion bookkeeping. `skills/04-review/concurrency/regression-guards/etcd-monitor-handle-not-aborted.md` does not promise synchronous destruction of caller-held handles; this finding does not add that promise. The ConfCenter bootstrap abstraction remains unchanged. Searches of current architecture-review artifacts and relevant review knowledge found no existing EtcdLockGuard completion finding. Historical task references do not authorize a broader lock or SDK redesign.

The evidence is static current source and read test bodies. No timing, throughput, server error rate, or exactly-once cleanup claim is made. Retry policy after partial failure, cancellation during acquisition before a guard exists, and runtime shutdown behavior remain separate contracts.
