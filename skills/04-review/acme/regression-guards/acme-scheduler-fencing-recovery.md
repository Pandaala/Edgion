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

## Manual trigger documentation boundary

`acme-manual-trigger-documentation` corrected the Chinese operator guide's
unsupported `edgion.io/trigger` annotation example and the nonexistent
`edgion-cli acme trigger` recommendation in both languages. Document the
authenticated `POST /api/v1/services/acme/{namespace}/{name}/trigger` instead.
The route classifier requires `acme-trigger` on `Service`; a newly generated
CLI token must first have its hash and permissions configured on the Controller.

`api::trigger_acme` calls `trigger_resource`, which enqueues `AdminTrigger`.
A successful response establishes enqueueing only, not resource existence,
eligibility, or certificate issuance. A missing or closed service sender returns
HTTP 503; contact the leader with a running ACME service. In `check_resource`,
Admin resets the persisted attempt count and bypasses a same-generation future
retry deadline, but still passes the normal eligibility gate. The Ready check
also remains in force: a certificate outside its renewal window is not reissued.
An active job coalesces the command as pending work rather than starting a
concurrent attempt.

`EdgionAcmeHandler::parse` compares source UID/generation, ResolvedRefs
status/reason, and listener attachments; it does not consume an arbitrary
metadata annotation as a command. Ordinary notifications and startup preserve
the same-generation attempt budget. `persisted_attempts` resets it for Admin
or a new source generation. The existing
`persisted_retry_budget_resets_only_for_admin_or_new_generation` unit test
asserts these distinctions; this documentation correction does not change
runtime behavior or add tests. Its source and test review is current, while
the existing scheduler regression evidence below concerns the earlier fixes,
not a claim of a fresh live CA/API-server run.

The operator procedure is maintained in the
[English](../../../../docs/en/ops-guide/gateway/tls/acme.md#manual-trigger) and
[Chinese](../../../../docs/zh-CN/ops-guide/gateway/tls/acme.md) guides.
Re-evaluate these instructions if the CLI gains an ACME command, the handler
gains a supported trigger input, or Admin admission, retry-reset, renewal-window,
or authentication behavior changes.

Fresh closure verification for this documentation correction: all nine
`AGENTS.md` pre-commit checks passed, including workspace check/clippy and
global agent-doc validation. No live CA/API-server test or full workspace
test run was performed for this documentation-only correction.

## Side-effect boundary

The attempt guard runs before account registration/persistence, order creation,
each challenge activation, finalization, challenge/lifecycle status writes, and
Secret/EdgionTls publication. Long CA polling and propagation waits select on
the same cancellation token. Existing target objects carry optional UID/resourceVersion
into their forced SSA update only after explicit API name/namespace match the
checked destination derived from the actual attempt. All active generated output
identities are validated before the first output IO. These identity checks do not
replace the fences or change adoption policy. The Lease and source resource are re-read around
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

## Scheduler clearing and the structural CRD

Clear the checkpoint by omitting `status.scheduler` from this manager's SSA
payload, keeping `status: {}`. The CRD declares a non-nullable object: explicitly
sending `scheduler: null` is rejected with HTTP 422, leaving a published
certificate stuck in the checkpoint-clear retry loop. Do not weaken the CRD or
use a broad status replacement to work around this contract. Preserve the source
UID/resourceVersion fence and the independent lifecycle/condition/challenge
writers. This relies on the documented single scheduler field-manager owner;
re-evaluate deletion semantics if another manager owns the same field.

The Ready-recovery regression must assert absence, not JSON indexing plus
`is_null()`, which also returns true for a missing field. Real API-server dry runs
in the Center alignment topology reproduced the null rejection and confirmed
that omission removes the scheduler while preserving Ready and certificate
expiry. Full runtime recovery/renewal evidence is tracked separately in the
sibling Center alignment task; a mock Kubernetes response alone cannot establish
SSA deletion behavior.

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
They do not run a real CA or Kubernetes API server. The non-Ready `nextRetryAt`
admission gate has no direct test: the only checkpoint test carrying a future
`nextRetryAt` is the Ready-recovery case, where the checkpoint is cleared and the
gate is skipped. Cover that branch before relying on it as verified behavior.

Re-evaluate this design if ACME work becomes multi-leader, certificate material
must survive a process crash before publication, or another controller begins
writing `status.scheduler`.
