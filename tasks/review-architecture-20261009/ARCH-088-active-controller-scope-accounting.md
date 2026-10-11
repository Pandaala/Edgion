---
id: ARCH-088
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-402
analyst: /root/verify_ip_union
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-088: Give active-controller accounting the controller future's lifetime

## Current ownership and concrete failure

edgion-controller/src/conf_mgr/sync_runtime/metrics.rs owns the existing unlabeled edgion_controller_active Gauge. Its public controller_started and controller_stopped methods require callers to balance two separate emissions.

FileSystem resource_controller.rs::run increments at line100 and decrements only at its normal tail, line564. Etcd's corresponding run increments at85 and decrements at344. Both event loops can suspend before reaching that tail, including while waiting for events or joining their worker after shutdown. Their actual parent controllers call handle.abort() for every resource-controller task after the watcher runtime returns: FileSystem controller.rs:167-172 and Etcd controller.rs:146-151.

A permitted shutdown interleaving is therefore: a resource controller has incremented and is suspended; the watcher observes the shared shutdown first and returns; the parent aborts the resource controller before its normal decrement. When Tokio destroys that aborted future, no matching decrement runs. An unwind after the increment has the same accounting problem. This is a source-level reachable cancellation scenario, not a claim that every shutdown takes that ordering.

The persistent Gauge can consequently overcount controllers that have already ended. A process that continues through reload can retain that stale contribution alongside replacement controllers. Controller startup installs the recorder before ConfMgr startup with idle eviction disabled, so exporter eviction does not repair or cause this mismatch.

## Existing mechanism and bounded improvement

Kubernetes already implements the needed ownership mechanism: its private ActiveController increments in start and decrements in Drop. run_sources acquires that guard after source/setup validation and retains it through event processing and cleanup. Errors before acquisition do not contribute; normal return, processing error, task cancellation and unwind after acquisition release the contribution when the future is destroyed.

Promote and reuse this existing small guard at the shared metrics owner instead of adding separate FileSystem and Etcd copies. Acquire it at the current accounting boundary in each backend. Replace the FileSystem/Etcd manual pairs and remove the Kubernetes-local duplicate. Keep the guard non-cloneable and let one acquired scope own exactly one increment/decrement pair. Remove the old tail decrements when adopting the guard so normal exits cannot decrement twice.

This changes the lifetime of existing accounting, not controller execution. Do not add a metric, label, log, timer, registry, worker, cancellation framework or request-path allocation. The guard is not proof that detached descendants have stopped, and task abort does not synchronously destroy a future before Tokio processes cancellation. Do not promise a decrement after process abort, termination or runtime destruction.

## Preserved contracts and distinguishing verification

Preserve the metric's name, unlabeled shape and recorder/cardinality policy. Preserve each backend's current start boundary and scope through its existing cleanup, including Kubernetes validation before acquisition. Leave one worker/queue per kind, initialization completion, inline status IO, existing retry policy, raw snapshots, leader handling, worker teardown and reload behavior unchanged.

Current queue, shutdown and resource-version unit-test bodies were read. FileSystem's actual duplicate-initialization test polls the run future and verifies worker ownership and orderly shutdown, but does not assert active-controller accounting. Etcd's current tests cover identity and malformed-resource transitions rather than run-future accounting. Kubernetes's cancellation/forced-abort test checks worker/raw-store cleanup, not this Gauge.

Future distinguishing coverage should exercise the shared production guard with isolated recording handles: normal completion, destruction of a started pending future, abort followed by observing task destruction, and unwind where supported. Each case must leave exactly the preceding baseline, while overlapping live scopes retain their separate contributions. An unpolled run future must not contribute. Cover FileSystem run wiring through its existing network-free event-loop fixture; preserve the current initialization and Kubernetes cancellation tests. Do not introduce a production client abstraction or real-service unit test merely to mock Etcd accounting.

## Original rules, deduplication and limits

The graded design rules require every Gauge increment to have a matching decrement, and the original metrics rules prefer Drop/RAII to enforce symmetry. This proposal reuses the already implemented Kubernetes mechanism rather than inventing another lifecycle abstraction.

ARCH043 concerns detached writable handles under Gateway exporter eviction. ARCH078 concerns missing authoritative process-state retention under that eviction policy. This candidate concerns an unbalanced existing Controller Gauge when an owning future ends; Controller's configured eviction policy is zero. The source-snapshot and single-worker decisions preserve their existing execution boundaries and do not authorize losing metric accounting on cancellation.

Classification: behavior-defect, P2. Static read-only analysis only; no source edits, tests, builds, service execution, network operations or Git actions occurred. No performance measurements, production incident or broader task-leak guarantee is claimed.
