---
name: acme-scheduler-fencing-recovery
description: ACME scheduling uses one actor, Lease fencing, and a CRD status checkpoint.
knowledge_type: regression-guards
---

# ACME scheduler fencing and recovery

## Required invariant

`acme-scheduler-trigger-admission` is closed by three cooperating mechanisms,
not by using the EdgionAcme CR as a lock:

1. One process-local actor owns all per-key scheduling state. Each key has at
   most one active job, one coalesced pending trigger, and one earliest deadline.
2. Every attempt captures and repeatedly validates the current Kubernetes Lease
   fence: Lease UID, holder identity, leader term, and transition count. It also
   validates service generation, source UID/generation, deletion, processed
   generation, conditions, and live listener proof.
3. `status.scheduler`, owned only by `edgion-acme-scheduler`, persists the source
   generation, attempt ID, consumed attempt count, attempt start, and next retry.
   The reservation must succeed before any CA operation begins.

The handler emits resource-change notifications for proof changes, authoritative
denial, and delete. A resource notification cancels pre-issuance work and leaves
one pending recheck. Once a certificate has been returned, it remains in memory
while publication revalidates source identity, ownership, and the same Gateway
UID/spec generation and listener identity/key set. A converged replacement proof
may change its epoch/digest because the issued Secret itself reloads the Gateway.
Admin commands take pending priority and alone reset the
budget without requiring a new spec generation.

## Side-effect boundary

The attempt guard runs before account registration/persistence, order creation,
each challenge activation, finalization, challenge/lifecycle status writes, and
Secret/EdgionTls publication. Long CA polling and propagation waits select on
the same cancellation token. Existing target objects carry UID/resourceVersion
into their forced SSA update. The Lease and source resource are re-read around
the authority decision; transient reads fail closed and wait inside the guard,
without discarding an order or registered credentials. Existing account Secrets
use the original UID/resourceVersion for merge-patch; after a conflict, valid
concurrent credentials win and an empty same-UID target can be retried. Replaced
or malformed account Secrets are never overwritten.

This is not exactly-once ACME and not a multi-object Kubernetes transaction. A
CA request accepted immediately before cancellation cannot be recalled. An
absent target can also be created concurrently after its read. Later authority
checks prevent further stale progress. Once a certificate has been returned,
publication and checkpoint clearing retry from the retained certificate and do
not submit a second order within that service generation.

## Recovery and classification

Startup lists all watched namespaces and retries transient list failures. It
feeds `StartupCheck`, which preserves the persisted retry budget and future
`nextRetryAt`. A confirmed source 404 deletes the actor state; other GET errors
retain it and schedule a control-plane recheck. A new spec generation ignores
the prior checkpoint. Successful publication clears the checkpoint. Failed
issuance persists lifecycle failure and the next retry checkpoint before the
actor becomes idle; persistence failure is retried without contacting the CA.
If the final reserved attempt was interrupted in `Issuing`/`Renewing`, recovery
persists `Failed` with an explicit exhausted-budget reason. A failed terminal
write schedules another control-plane check and never reserves a sixth attempt.

`LeaderFenceValidation` distinguishes `Current`, authoritative `Lost`, and
`Uncertain(kube::Error)`. It checks local leadership both before and after the
Lease GET so a local step-down during the read cannot validate an old attempt.

## Regression anchors

- `edgion-controller/src/services/acme/service.rs`
- `edgion-controller/src/services/acme/acme_client.rs`
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/leader_election.rs`
- `edgion-resources/src/resources/edgion_acme.rs`
- `config/crd/edgion-crd/edgion_acme_crd.yaml`

The in-memory Kubernetes transport tests cover Secret-publication proof feedback,
operation-guard transient reads and cancellation, exhausted interrupted-attempt
terminal-write retries, and existing-account Secret conflicts (concurrent valid
credentials, still-empty target, replacement UID, and corrupt credentials).
They do not run a real CA or Kubernetes API server.

Re-evaluate this design if ACME work becomes multi-leader, certificate material
must survive a process crash before publication, or another controller begins
writing `status.scheduler`.
