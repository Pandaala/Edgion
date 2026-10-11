---
id: ARCH-009
status: candidate
kind: architecture-debt
priority: P3
primary_partition: PART-249
independent_verification: confirmed
---

# Keep credential-admission projection inside the prepared AI attempt

## Current ownership and concrete maintenance cost

`ai/runtime/dispatch.rs::prepare_ready_attempt` resolves the selected backend,
credential, effective policy, observation, usage state and converter reservation.
Its PreparedAiAttempt exposes provider/pricing, Redis reference/failure policy,
credential limits, token estimates, usage/EOS handles and limiter core as separate
public fields.

`upstream_peer_http` reconstructs AiAttemptAdmissionInput in
`routes/http/proxy_http/pg_upstream_peer.rs:570`. It copies the prepared fields
and extracts namespace, backend UID, credential alias, outlier handle, adaptive
state and logical circuit breaker from the selected backend. The actual path is
preparation, transport reconstruction, admit/reservation, endpoint resolution,
context commit, and actual upstream-writing dispatch mark.

Adding a reservation policy field crosses the selected runtime, prepared DTO,
HTTP assembler and admission input. The HTTP loop must understand both selected
credential and logical backend policy. Prepared fields repeat data retained by
the selected runtime or private request state: two projections represent one
selected attempt. No wrong-policy behavior is established.

## Bounded improvement and legitimate responsibilities

Let the prepared attempt project the canonical AiAttemptAdmissionInput through
a private runtime-owned method, or retain an already-prepared input. Derive the
policy from the retained selected backend and credential. Remove copied public
fields without remaining consumers, keeping required per-plan output bounds
inside the prepared owner.

HTTP transport keeps its existing await/result/deadline/endpoint loop, skip
scope, reservation release/drain and delayed context commit. Admission, endpoint
resolution and actual dispatch remain distinct. Moving the whole peer loop or
merging admission with connection establishment would obscure these duties.
Keeping the current explicit assembler is locally straightforward but preserves
policy reconstruction in transport and duplicate field threading.

Moving the projection must preserve allocations: no extra credential/body copy
or Arc/String cloning for convenience. Backend key representation is unchanged,
so this root is separate from ARCH-008.

## Invariants, risks and future validation

Preserve request-stable backend/credential generations and authorization;
outlier, adaptive and circuit-breaker state must come from the same selected
attempt. Preserve Redis failure policy, RPM/TPM limits and token-bound precedence.
Admission remains before endpoint resolution and context commit after successful
peer construction. Recheck deadlines before preparation and after I/O, preserve
predispatch release/drain and credential-versus-backend skip scope.

Mark dispatch only immediately before upstream writing: pre-connect failures
consume no scheduler attempt or retry-budget denominator. Preserve unary budget
release, baseline restoration, BodyStorage generations, processor order and
cancellation. Ownership/borrowing across preparation and admission await must be
proved in a future implementation.

Retain admission-policy, deadline, endpoint-skip, retry-budget and converter
admission tests. Focused production projection assertions should verify selected
credential/backend policy without widening visibility for mocks. Any runtime
failover acceptance should use existing shared topology. No implementation or
executed validation is claimed.

## Original context and review status

The analyst read original AI routing/quota, predispatch deadline, failover,
native accounting, requested-model quota, monotonic usage, gateway-cut settlement,
translator-only accounting, SSE failure and typed terminal decisions. Actual
admission/endpoint/context/dispatch consumers and source tests were inspected.
No decision requires HTTP transport to reconstruct credential-admission policy.

This is separate from ARCH-001 session capability growth, ARCH-005 transport
metadata and ARCH-008 compiled lookup keys. Exact source/evidence bindings and
the independent decision are recorded in STATE.json and partition reports.
