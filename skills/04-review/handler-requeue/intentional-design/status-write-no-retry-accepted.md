---
name: status-write-no-retry-accepted
description: Use when reviewing controller status-write findings; covers why a failed persist_k8s_status() only warns and is not re-queued (no requeue_with_backoff), and why that is an accepted tradeoff rather than a consistency bug.
knowledge_type: intentional-design
---

# Status-Write Failure Is Not Re-queued — DESIGN-TRADEOFF

## Conclusion

When `persist_k8s_status()` fails with a non-404 error, the worker logs a
`tracing::warn!` and does **not** re-enqueue the resource. This is intentional,
not a missing-retry bug. Findings of the form "status write failures are
silently dropped / no eventual consistency / `observedGeneration` left stale /
`kubectl wait --for=condition` may hang / should re-queue on reconcile error
like controller-runtime" are **accepted tradeoff (controller-01, 2026-05-29)**,
closed without code change.

Fix suggestions of the form "re-queue the key via `enqueue_after` on any non-404
error" or "reintroduce an exponential `requeue_with_backoff` like
controller-runtime" are **not accepted**.

The shared raw-input resourceVersion precondition also follows this policy: a stale-snapshot
409 ends the current attempt, with no GET/retry or failure-triggered requeue. Missing/empty
input versions fail locally through the existing error path. This retry decision never
authorized unconditional writes or lost foreign updates; see the separate
[snapshot concurrency correction](../../status/regression-guards/kubernetes-status-snapshot-concurrency.md).
The `route-status-concurrent-writer-preservation` follow-up proposed an immediate
fresh-read, merge, and bounded retry for same-UID/same-generation Route conflicts.
That improvement was declined: preserving the foreign write and reconciling on a
later event is the chosen behavior. A successful foreign status write normally
produces a Route watch event, which can enqueue the key even while its previous
attempt is in flight. Recovery still has no strict time bound if that event is
not delivered and no later source LIST or other requeue occurs.

## Core Rationale

**1. Recovery depends on a later reconciliation opportunity**

A failed status PATCH can require operator action or recover after a transient
failure. Neither class guarantees a subsequent event:

- **A configuration/setup error** — e.g. an admission webhook rejecting the
  status subresource, a CRD schema mismatch (422), or RBAC denial (403) on the
  status subresource. These require operator action to fix the underlying
  config. Editing the resource can produce a `MODIFIED` event and another
  reconcile. Fixing RBAC or a webhook alone need not change the resource, so
  it does not by itself guarantee a status update.
- **A dependency change** — the resource depends on a Secret/Service/Gateway/etc.
  that is not yet ready. The existing cross-resource
  requeue trigger paths (see
  `skills/01-architecture/01-controller/06-requeue-mechanism.md`): when the
  referenced resource arrives, the dependent is re-enqueued and its status
  recomputed. This is a reconciliation trigger, not a classification of PATCH
  failures; status persistence can still fail independently.

**2. The `requeue_with_backoff` mechanism was deliberately removed**

`skills/01-architecture/01-controller/04-workqueue.md` records that
exponential-backoff retry was removed because **this layer cannot accurately
distinguish transient from permanent failures**. A blind "re-queue on any
non-404" would loop forever on genuinely permanent errors (403 RBAC, 422
validation, 409 conflict that does not self-resolve) — a permanent requeue
loop is strictly worse than a recoverable stale status. controller-runtime
accepts that infinite-with-backoff retry; Edgion deliberately chose the
opposite tradeoff for these control-plane status writes.

**3. Quiet resources can retain stale status without a time bound**

A transient apiserver error (429/503/TCP timeout) can leave status stale while
the watch remains healthy and the resource has no further relevant events.
This is the accepted cost of the no-automatic-retry decision; no frequency
measurement or bounded recovery guarantee is claimed here.

A later `MODIFIED`, cross-resource requeue, leadership-acquisition `requeue_all`,
or successful source LIST provides another attempt when computed status differs
from raw persisted status. Source relists enqueue all current keys, including
identical resourceVersions, plus removed keys. They preserve ServerCache and
the existing worker. A source relist does not retry unrelated sources or kinds;
a reconnect that resumes WATCH without LIST does not cause resync. The former
global rebuild policy's incidental broader recovery is no longer applicable.

## Fix Suggestions Not Accepted

- "Re-queue via `enqueue_after` on any non-404 error" — loops forever on
  permanent errors (403/422/409), reintroducing the failure mode the
  `requeue_with_backoff` removal was designed to avoid.
- "Reintroduce exponential `requeue_with_backoff` like controller-runtime" —
  same permanent-failure-loop problem; this layer cannot classify
  transient-vs-permanent, which is the documented reason it was removed.
- "Surface a status-write-failure condition / operator-visible indicator" — the
  `record_status_write_error` metric already exists in `persist_k8s_status`;
  adding a per-resource status condition for a write that itself failed is
  circular (the condition write would face the same apiserver).

## Re-evaluation Triggers

Re-open this decision only if:

- A reliable, cheap transient-vs-permanent classifier becomes available at this
  layer (e.g. honoring `Retry-After` on 429/503 with a hard bounded retry count
  that gives up to permanent-error handling), so a bounded retry can be added
  without risking a permanent-error loop.
- A production incident shows the purely-transient stale-status window causing
  real operator-visible harm that the next-event self-heal does not resolve in
  acceptable time.

## Reference Cases

- `route-status-concurrent-writer-preservation` — immediate conflict recovery
  declined after the resourceVersion guard already prevented foreign-entry loss;
  the remaining cost is delayed Edgion status convergence without a time bound.
- `tasks/review-envoy/controller-01-status-write-no-retry.md` (2026-05-29) —
  finding closed as accepted tradeoff.
- `skills/01-architecture/01-controller/04-workqueue.md` — records the
  `requeue_with_backoff` removal and its rationale.
- `skills/01-architecture/01-controller/06-requeue-mechanism.md` — the six
  cross-resource requeue trigger paths that self-heal reference errors.
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/resource_controller.rs`
  (`persist_k8s_status`, `spawn_worker`, `run_init_apply_once`, `run_sources`).
- `kubernetes-namespace-initial-relist-generation`: source-scoped snapshot
  redesign narrows recovery scope while retaining the accepted retry policy.
  Implementation verification is recorded in the source-snapshot regression guard.
