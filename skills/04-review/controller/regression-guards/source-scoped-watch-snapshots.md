---
name: source-scoped-watch-snapshots
description: Use when reviewing Kubernetes namespace LIST replacement, one-time bootstrap, runtime resync, or resource-epoch teardown.
knowledge_type: regression-guards
---

# Source-scoped watch snapshots and one-time bootstrap

## Conclusion and scope

The approved Kubernetes correction isolates each native Writer/Store by watch
API scope and reuses one workqueue/worker per kind. Production implementation,
targeted tests, both-mode real Kubernetes execution and required workspace
checks passed. Independent final implementation/test/knowledge review returned LGTM.

`SourceState::ever_committed` means that a source completed at least one LIST
in the current run. A new LIST resets staging, not that fact or the committed
snapshot. `ResourceController::run_sources` freezes the complete committed union
for one initialization replay, then starts one worker. Later source commits
enqueue old and current keys through `on_event_key`, including unchanged RVs.
Processor and worker share the same union `RawStore`.

Namespace's first snapshot is an explicit startup prerequisite; subsequent
metadata changes defer Selector refresh until Gateway initialization completes.
Disabled kinds cannot bypass the initial Namespace publication gate. Ordinary
relists retain worker, ServerCache, and configuration-sync identity.

## Rationale and evidence

The original `merge_namespaced_watchers` suppressed A's repeated Init while B
was still loading, leaving A's completion flag set. B could then emit global
completion, followed by A's InitApply and another completion. A single native
reflector consumed the latter before the Controller duplicate guard and could
replace the union with only A's snapshot. The guard prevented a second worker
but not partial raw publication or concurrent direct init processing.

The canonical one-worker and runtime enqueue rules are in root `AGENTS.md`
and `skills/00-design-rules.md` section H. Native kube Writer staging is reused;
there is no new object cache or business queue. Source identity and lifecycle
transitions are validated before Writer mutation. Empty initial snapshots count
as complete, while interrupted LISTs never publish their partial staging.

Persistent processed state requires eligibility-loss cleanup: when a new raw
object fails namespace/handler filtering, delete any old cached object through
the existing hooks but retain the new raw object. Other handler Skip outcomes
keep their existing meaning. UID recreation still requires clear-before-add
side effects and dropped-dependent requeues.

Resource descendants are registered with TaskTracker before running and owned
by JoinSet/abort-on-drop handles. Cleanup withdraws config-sync, cancels, joins
or aborts and joins top-level spawners, then closes and awaits tracked descendants
before reset. Epoch cancellation is distinct from process shutdown: leader-only
loss waits for a future acquisition; all-serve leadership changes retain the
resource epoch. Native source teardown clears both committed and staging data,
including when diagnostic reader Arcs remain registered.

All-serve classifies process shutdown after teardown, overriding a phase failure
or normal child completion that won the event race. Its outer loop and retry
backoff also observe shutdown, preventing a replacement epoch after shutdown.
Tests exercise pending Namespace bootstrap through the serving flow and outer
lifecycle, shutdown arriving during descendant drain, and preflight retry backoff.

## Alternatives and limits

Multiple per-source workers plus a ServerCache lock would not serialize handler
side effects and violate the canonical per-kind processing model. Resetting
completion flags alone cannot retract already processed abandoned objects.
Source-scoped complete commits fix that ownership boundary.

Raw replacement is atomic per source; processed state converges eventually.
An old worker read can publish briefly before a dirty requeue processes the new
raw object. Inline status IO and the existing unguarded status MergePatch
UID/resourceVersion race are not solved here. Status-write recovery is limited
to relevant events and successful source resync, with no time bound; see
[the accepted retry policy](../../handler-requeue/intentional-design/status-write-no-retry-accepted.md).

Queue backpressure is awaited without native-store locks. Cancelled partial
delivery abandons the entire epoch. Bootstrap does not enqueue source items
before its worker exists; this does not establish unlimited handler-generated
requeue capacity. A non-yielding task has no bounded abort/join guarantee, and
runtime reuse must remain blocked while config-sync is unpublished.

## Verification and re-evaluation

The approved S1-S20 matrix is in
[the design spec](../../../../tasks/todo/kubernetes-namespace-initial-relist-generation/01-design.md).
Executed targeted checks and final results are in
[verification evidence](../../../../tasks/todo/kubernetes-namespace-initial-relist-generation/04-verification.md).
The original defect was derived
from source/event ordering, not a production incident or live-cluster reproduction.

Subsequent real-API verification passed 8 checks in each HA mode, including
source LIST interleavings, raw/processed UID convergence, reload and real owned-
Lease loss/acquisition. The final run observed 42 leader-only and 65 all-serve
availability samples without errors and removed all 6 owned namespaces. These
are sampled control-plane checks, not data-plane HTTP or exact worker TaskId
measurements. Native source/lifecycle tests and review establish one-worker
ownership and cancellation/drain behavior. Protocol ERROR Status 410, rather
than a WATCH handshake HTTP 410, triggers the locked kube-runtime's fresh LIST.
The live run predates the Astra shutdown correction; its process cleanup may
force termination and is not evidence for graceful shutdown classification.
The later lifecycle regression tests cover that correction. Bootstrap incremental
coverage uses non-cancelling changes: a retained addition, an updated UID and a
separate deletion must appear in the first processed union.

Re-evaluate when native Writer semantics, source overlap, watch strategy,
bootstrap hooks, queue cancellation, Namespace readiness, task spawning, or
status-retry policy changes. Debug raw counts/keys must include filtered objects
and apply only verified fixture exemptions in cache-residue validation.

## Resolution references

- `kubernetes-namespace-initial-relist-generation` (2026-09-07): real lifecycle
  defect; source-scoped redesign approved and implementation authorized on
  2026-09-08. Closed as fixed after implementation, required verification and
  independent final review passed.
- `initdone-double-spawn-second-worker`: earlier completion guard remains valid;
  its historical verification does not prove the broader source lifecycle.
