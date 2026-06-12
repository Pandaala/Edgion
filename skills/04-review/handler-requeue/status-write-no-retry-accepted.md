---
name: status-write-no-retry-accepted
description: Use when reviewing controller status-write findings; covers why a failed persist_k8s_status() only warns and is not re-queued (no requeue_with_backoff), and why that is an accepted tradeoff rather than a consistency bug.
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

## Core Rationale

**1. Non-404 write failures decompose into two self-healing classes**

A status PATCH that fails with a non-404 error is, in practice, one of:

- **A configuration/setup error** — e.g. an admission webhook rejecting the
  status subresource, a CRD schema mismatch (422), or RBAC denial (403) on the
  status subresource. These require operator action to fix the underlying
  config. When the operator fixes it, the spec edit produces a `MODIFIED`
  event → the resource is re-processed → the status is rewritten. The
  controller does not need to blindly retry; retrying before the operator acts
  would only spin.
- **A reference error** — the resource depends on a Secret/Service/Gateway/etc.
  that is not yet ready. This is already covered by the six cross-resource
  requeue trigger paths (see
  `skills/01-architecture/01-controller/06-requeue-mechanism.md`): when the
  referenced resource arrives, the dependent is re-enqueued and its status
  rewritten.

**2. The `requeue_with_backoff` mechanism was deliberately removed**

`skills/01-architecture/01-controller/04-workqueue.md` records that
exponential-backoff retry was removed because **this layer cannot accurately
distinguish transient from permanent failures**. A blind "re-queue on any
non-404" would loop forever on genuinely permanent errors (403 RBAC, 422
validation, 409 conflict that does not self-resolve) — a permanent requeue
loop is strictly worse than a recoverable stale status. controller-runtime
accepts that infinite-with-backoff retry; Edgion deliberately chose the
opposite tradeoff for these control-plane status writes.

**3. The residual purely-transient case is rare and recoverable**

The one case neither class above covers is a purely transient apiserver error
(429/503/TCP timeout) while the watch stream stays healthy AND no concurrent
spec change / leader transition / watcher reconnect follows. In that window the
status is left stale. This is accepted because it is (a) rare (requires the
conjunction of a transient write failure with a quiescent resource and a healthy
stream), (b) recoverable — the next relevant event (`MODIFIED`, cross-resource
requeue, `requeue_all` on leader transition, or watcher reconnect / full LIST)
rewrites the correct status, and (c) exactly the slice of failures that "cannot
be distinguished from permanent at this layer" — the reason backoff was removed.

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

- `tasks/review-envoy/controller-01-status-write-no-retry.md` (2026-05-29) —
  finding closed as accepted tradeoff.
- `skills/01-architecture/01-controller/04-workqueue.md` — records the
  `requeue_with_backoff` removal and its rationale.
- `skills/01-architecture/01-controller/06-requeue-mechanism.md` — the six
  cross-resource requeue trigger paths that self-heal reference errors.
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/resource_controller.rs`
  (`persist_k8s_status`, worker at ~537-567, `run_init_apply_once` at ~708).
