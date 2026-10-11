---
id: ARCH-092
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-394
analyst: /root/analyze_conversion
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-092: Tear down a ready local configuration generation when its backend error channel closes unexpectedly

## Problem and current ownership

`CenterBase::run_lifecycle_gated` treats a closed backend error channel as permission to keep waiting for process shutdown before performing iteration cleanup. If the actual top-level FileSystem or etcd controller task ends unexpectedly after readiness without sending an error, the configuration generation remains advertised as ready even though that task is gone. The selected handler also stops observing reload requests and leadership-loss events.

The shared owner creates a oneshot error channel and stores the backend `JoinHandle` without observing its completion (`non_kube_common.rs:92-99,239-244`). It publishes the ConfigSyncServer after registry readiness (`266-275`). In the post-ready select, an `Err(_)` from `error_rx` directly awaits `global_shutdown.wait()` before producing a LoopAction (`290-296`). Cleanup that signals the iteration, stops the memory credential service, withdraws the CSS, clears its old stores, aborts the controller and resets the runtime occurs only after that await returns (`302-323`).

The actual FileSystem and Etcd `make_handle` closures move `error_tx` into the spawned controller task and send it only for `ctrl.run(...).await` returning Result::Err (`file_system/center.rs:138-162`; `etcd/center.rs:134-157`). A task unwind drops the sender without sending. Pinned Tokio 1.53.1 completes the oneshot on Sender::drop and makes Receiver return RecvError when there is no value. This is a concrete handled channel outcome at an existing task-ownership boundary; it does not require a new event sequence or early runtime-watch startup.

## Observable consequence and limits of the witness

Assume the backend task ends unexpectedly after the CSS is published while process shutdown remains false. The resource registry's ready flags and the published CSS are unchanged, so `CenterBase::is_ready` remains true (`non_kube_common.rs:137-139`) and the actual Admin readiness chain can still return 200. A reload can enter the capacity-one channel, but that channel is no longer selected while the closed-error arm waits. In etcd mode, subsequent leadership loss likewise cannot run this loop's iteration cleanup; native election/demotion and its independent term cancellation are separate owners and are not claimed to stop functioning.

The CLI deadman observes the enclosing `ConfMgr::start_with_shutdown` task (`cli/mod.rs:596-641`). Here that lifecycle future remains pending inside the inner shutdown wait, so the deadman does not observe this completed child. No particular operator input or native SDK panic is asserted, and no production incident is claimed. The witness is conditional on unexpected completion of the current spawned backend task, including panic. It does not claim detection of a live but stalled task or failure of every per-kind worker.

## Bounded improvement

Classify backend error-channel closure before entering any shutdown wait. If process shutdown is already requested, retain the normal Shutdown path. Otherwise produce the existing failure action with a fixed, non-sensitive description and execute the existing iteration cleanup immediately. The existing reported-error policy may then wait for shutdown after readiness and the old CSS have been withdrawn, just as it already does for a delivered error. Do not start a new generation automatically or change reported-error recovery policy as part of this fix.

Observing the stored backend JoinHandle could additionally distinguish panic from normal unexpected return, but a general supervision framework, new channel or task registry is unnecessary to close this specific cleanup bypass. If the implementation chooses handle observation, it must preserve a single owner of completion and avoid double cleanup when a delivered error and task completion coincide.

## Preserved invariants and alternatives

Preserve normal shutdown when sender closure accompanies process shutdown, the current reload path, the existing Error action's cleanup-before-wait contract, leadership-gated startup and native election-owned demotion. Preserve withdrawal of CSS readiness before clearing old caches, current generation-change signaling, memory credential service cleanup, selective runtime reset, phased provider/Gateway/dependent initialization and one workqueue/serial worker per resource kind. This adds no API IO to handlers or asynchronous status writer.

Leaving the selected arm waiting first retains a ready generation without its backend task. Blindly restarting the child would add recovery policy and could repeatedly re-enter a broken state; it is outside scope. A progress watchdog would address a different live-task hang. Logging ownership and restrained observability remain unchanged; a bounded lifecycle failure reason belongs to the existing error path, with no raw resource payload or additional traffic metrics.

## Original decisions, tests, and verification limits

The original Kubernetes liveness/supervision decision explicitly separates completed/panicked children from startup and post-ready hangs, and observes failures at spawned-task boundaries. It is scoped to Kubernetes flow; it neither supplies supervision for this local backend loop nor accepts delaying cleanup after completed local-child failure. Its rejection of automatic leader-job restart remains relevant, and this proposal does not add it. The single-worker and initial-document ownership decisions remain intact.

The complete existing `non_kube_common_test.rs` covers provider phase membership and ungated FileSystem startup, not a post-ready closed error channel. Future focused coverage should publish a ready local generation, close the actual iteration error sender without process shutdown, and verify CSS/readiness withdrawal and entry into existing failure cleanup. Include reported errors, simultaneous orderly shutdown/closure and reload as controls. Synchronize on lifecycle events rather than timing sleeps, and do not invent a broad backend mock framework solely for this case.

All evidence is static source/test inspection. No tests, applications, build, Git or network operations ran. This does not establish a particular crash trigger, supervise all descendants, revise teardown-drain guarantees, restore automatic reload after the existing Error policy, or promise recovery from live-task stalls.
