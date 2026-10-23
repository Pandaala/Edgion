---
name: prepared-attempt-owns-admission-input
description: Use when reviewing prepared AI credential admission policy, retained generations and HTTP peer sequencing.
knowledge_type: regression-guards
---

# Prepared AI attempts own credential-admission projection

## Conclusion and scope

`PreparedAiAttempt::admission_input` is the production projection of
`AiAttemptAdmissionInput`. It derives identity, credential limits, Redis reference
and failure policy, limiter, outlier, adaptive state and logical circuit breaker
from its retained `SelectedAiBackend`. Provider, pricing and usage come from the
same private pending request context that commits after successful peer creation.
The EOS latch and resolved input/output token bounds remain private prepared state.

HTTP transport asks for the projection and retains the existing admission await,
deadline, endpoint, skip and context-commit control flow. It does not assemble
credential/backend policy. Request output bounds precede backend defaults and
zero fallback; input estimation keeps saturating arithmetic. Invalid credential
indices and calls after context commit fail with `InvalidConfiguration`, rather
than admitting without limits or panicking.

## Rationale and evidence

ARCH-009 identified policy threading across preparation, a public prepared DTO,
the HTTP assembler and the admission contract. A reservation-policy addition now
changes the runtime projection and admission contract without changing transport
or adding another prepared policy copy. The existing owned input representation
allows its source borrows to end before admission awaits; it introduces no extra
credential/body copies, locks or request allocations. Earlier redundant prepared
Redis-reference, limit and limiter copies are removed. Admission now copies the
backend key from the selected logical target's cold-compiled identity, retaining
owned input across the await. ARCH-009 initially left key formatting at admission;
a later data-plane optimization reuses the ARCH-008 prepared key instead.

Canonical sources are `edgion-gateway/src/ai/runtime/dispatch.rs`,
`edgion-gateway/src/ai/runtime/attempt_runtime.rs` and
`edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs`. The
[gateway architecture](../../../01-architecture/02-gateway/00-overview.md#ai-implementation-boundary)
describes the boundary. The selected runtime generation remains pinned; no global
lookup refreshes policy during projection.

## Alternatives and limits

Keeping the explicit HTTP assembler was locally simple but preserved policy
fanout and duplicate field threading. Moving the entire peer loop into admission
would hide legitimate transport responsibilities. Neither new borrowed admission
APIs nor lifecycle wrappers are needed for this ownership change.

Admission precedes endpoint resolution and context commit. Deadline checks remain
before preparation and after I/O; credential/backend skips and predispatch
release/drain remain distinct. Dispatch still starts immediately before upstream
writing, preserving retry-budget and scheduler accounting. Baseline restoration,
BodyStorage generation, response processor order and unary reservation ownership
are unchanged. Cancellation retains existing RAII probe/breaker cleanup and Redis
reservations' conservative TTL fallback; RAII does not promise immediate Redis IO.

## Verification and re-evaluation

Private dispatch tests exercise real preparation through an in-memory Pingora
session. `prepared_admission_projects_selected_credential_and_retained_generation`
distinguishes two credential policies, replaces the plan candidate after preparation,
and checks shared limiter/usage/EOS/adaptive/breaker identities and outlier behavior.
`prepared_admission_preserves_token_bound_precedence_and_saturation` covers explicit,
zero, default and absent output bounds plus saturating input estimation.
`prepared_admission_rejects_invalid_selection_and_committed_context` guards checked
failure. The deferred-protocol-header skip/retry test uses the canonical projection
with its explicit test breaker override. Existing admission, deadline, endpoint,
retry-budget and converter tests remain in place.

At ARCH-009 acceptance, focused Gateway library tests passed: dispatch 34,
attempt runtime 23 and HTTP upstream peer 101. Workspace check and clippy
(`--workspace --all-targets`), formatting, agent-document consistency,
unit-test layout and Gateway no-tracing checks passed. Independent AIMPL design
and final result reviews approved the implementation; cleanup verified that the
current source and tests still match the approved content hashes. These are
completion evidence, not new test executions during task cleanup.

These structural and focused tests do not establish live Redis cancellation behavior, shared-topology
failover acceptance or measured throughput/allocation improvements. Re-evaluate
when selection becomes mutable across admission, admission ownership changes,
policy gains generation-sensitive fields, or transport ordering changes.

## Resolution references

- ARCH-009 — prepared AI admission-input ownership; bounded runtime projection.
