---
name: acme-service-shape
description: Decide on ACME `service.rs` phase splitting and global dependency-injection findings after the actor scheduler refactor.
knowledge_type: intentional-design
---

# ACME service.rs Shape — Audited Tradeoff

**Audit date**: 2026-04-26
**Linked issue**: P1-05 (closed, only the code-defect parts fixed; no architectural refactoring)
**Scheduler re-evaluation**: 2026-09-21 (`acme-scheduler-trigger-admission`)

DNS-01 was removed on 2026-09-09. DNS-specific references below are historical; the service-structure decisions continue to apply to HTTP-01.

## Already Fixed (former real defects)

- Status write-back was silently dropped → now `if let Err(status_err) = update_acme_status(...)` logs `tracing::warn!` (service.rs:395-409)
- Non-UTF-8 DNS credentials silently fell back to base64 → now explicit `String::from_utf8(...).map_err(...)?` returns Err (service.rs:751)

These are the defect categories corrected in that historical audit, not an
exhaustive list of valid ACME findings. The accepted choices below concern service
structure and SSA ownership only; they do not exempt newly demonstrated security
or correctness defects. DNS provider error disclosure is covered separately by
[the diagnostic redaction guard](../superseded/dns01-retired.md).

## Fix Suggestions Not Accepted (after this audit)

### 1. "`execute_http01_flow` / `execute_dns01_flow` are 90 lines too long; should split into phase sub-functions"

- ACME protocol is a naturally linear flow: account → order → challenge → finalize → certificate; each step depends on the previous.
- The split sub-functions are each called once by this flow only, with long parameter lists (client, acme, status, challenge state…); readers would jump back and forth among 5–6 files.
- "Linear story sliced horizontally" is negative-value, hitting a failed acceptance against the triple in `architecture/size-and-similarity-heuristics.md`.
- **Verdict**: split not accepted.

### 2. Scheduler orchestration was re-evaluated

- The former detached-task scheduler was replaced after concrete correctness
  evidence: duplicate timers, restart retry loss, leadership handoff, and stale
  attempt publication could not be repaired reliably with local checks.
- One actor now owns per-key state while `check_resource` and `run_attempt` are
  typed jobs. This exception was justified by state-machine invariants, not file
  length or indentation. Purely cosmetic phase splitting remains unaccepted.
- See the [scheduler fencing guard](../regression-guards/acme-scheduler-fencing-recovery.md).

### 3. "`ACME_SERVICE: Mutex<Option<AcmeServiceHandle>>` global singleton should switch to `Arc<...>` injected into HandlerContext"

- The Controller is a leader-elected single-active model; multi-instance concurrency is not a design goal.
- The only pain is unit-test inconvenience, but the ACME flow bottleneck is K8s API + external ACME server; switching instantiation method still cannot test it; it just moves the mutex.
- The change cascades to HandlerContext + all handler signatures; benefit is illusory.
- **Verdict**: DI refactoring not accepted. If in the future ACME really needs unit tests or multi-tenant splitting, file a new targeted issue.

### 4. "`update_acme_status` / `patch_active_challenges` use `PatchParams::apply(...).force()` unconditionally; should drop `.force()` or implement try-then-force-on-409"

- The ACME service has three dedicated SSA writers/managers — `edgion-acme-controller` (lifecycle fields), `edgion-acme-challenges` (active challenges only), and `edgion-acme-scheduler` (the recovery checkpoint). This is not an inventory of every status writer: the generic resource handler also sends a Merge Patch of its observed status.
- The managers do not overlap: lifecycle serialization omits empty challenge and absent scheduler fields; the challenge and scheduler patches explicitly contain only their owned field.
- Active attempts now validate the live Lease fence and source UID/generation around side effects. This is API-server fencing, but it is still not a transaction across the source, Secret, EdgionTls, and status objects. See the [scheduler fencing guard](../regression-guards/acme-scheduler-fencing-recovery.md) and [snapshot concurrency guard](../../status/regression-guards/kubernetes-status-snapshot-concurrency.md).
- The unconditional `.force()` is the project-wide convention for SSA writes against Edgion-owned resources (see `conf_center/kubernetes/storage.rs:325` for the same pattern with `edgion-controller`); changing it only here would diverge from that convention.
- "Drop `.force()`" introduces an unhandled 409 path on the critical status write-back route. "Try-then-force-on-409" introduces a retry/state-machine layer to all 5 call sites for a hypothetical future risk that requires a third-party field manager to ever land on `EdgionAcme/status`.
- **Verdict**: SSA `.force()` fix suggestions on `update_acme_status` / `patch_active_challenges` are not accepted. Re-evaluate only if a third-party controller is actually introduced that writes the same status fields.

## When to Re-Evaluate

- ACME service really needs multi-instance support (e.g., multi-tenant isolation) → re-evaluate #3.
- A real unit-testable logic separation appears (e.g., extracting the ACME state machine into a pure-function module) → re-evaluate the split value of #1, #2.
- New swallowed errors / silent fallbacks appear (status write-back, external IO, key parsing) → file as a normal finding under "Already Fixed".
