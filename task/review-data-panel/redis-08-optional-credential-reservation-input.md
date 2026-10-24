# Optional credential-reservation input ownership

Status: deferred representation/API decision for user review. No runtime change.
Source baseline: `77ef8fc1e`.

## Evidence

`PreparedAiAttempt::admission_input` copies namespace, backend UID and the optional
Redis reference, and clones the pool's limiter Arc, including when the selected
credential has no limits. In `AiAttemptRuntimeState::admit`, those fields are used
only by the reservation path. String copies and Arc reference-count operations
are distinct costs; an absent reference or empty String need not allocate.

Admission first checks active-attempt/accounting-overflow state, then acquires a
logical circuit-breaker permit, then admits the credential outlier handle. Only
after those gates does `limits: None` skip reservation. `Some(limits)` enters
reserve even if its individual fields are empty; that runtime distinction does
not assert an empty-limits operator configuration is valid. Redis failure policy
applies to reservation errors, not earlier rejection or quota denial.

`reserve` reads the Redis reference, resolves a healthy client, constructs the
canonical reservation keys and invokes the shared limiter under its timeout.
It also uses credential alias and token bounds: the fields identified above are
a reservation-only subset, not the complete reservation input. Reserved/Duplicate
outcomes retain the limiter Arc in a settlement lease; denial and errors do not.

Backend display name, credential alias, usage, EOS and applicable accounting and
health ownership remain needed by the runtime even without a Redis reservation.
The HTTP attempt loop retains the prepared target across admission, resolves the
peer only after admission and then commits context. This makes borrowing a design
candidate, not proof that every current owned field can simply become a reference.

## Decision alternatives

- Keep the current flat owned input if avoiding a second representation and
  lifetime contract is worth its cost.
- Project one optional owned reservation input only when credential limits are
  present. Define the authoritative association between limits, selected identity,
  Redis policy and limiter; do not create contradictory independent options or
  replace required identity values with empty placeholders.
- Borrow reservation-only data from the retained selected generation through an
  explicitly reviewed admission interface. Prove the source survives the await,
  cancellation, peer failure and context commit without widening a lock/guard
  lifetime. Successful settlement leases still need independent ownership.

No alternative is selected. Do not skip breaker/outlier admission on the no-limits
path, refresh current global policy instead of the captured generation, redesign
Redis client lookup, or add a cache, new limiter, schema knob or logging surface.
The existing limiter borrow inside reserve and prepared backend-key/credential-name
optimizations remain separate and do not resolve this input representation choice.

## Acceptance before implementation

Preserve absent versus present limits, missing Redis references, client health,
fail-open/closed policy, active-attempt and overflow denial, breaker refusal,
missing/ejected/probe outliers, and their exact precedence. Preserve credential
alias/token-bound inputs, key framing, TTL calculation, operation timeout, Reserved,
Duplicate and denied result mapping, and owned bounded attempt-log identity.

Verify selected-credential and retained-generation behavior across publication,
including different credential limits. Exercise no-limit and limited admission,
deadline expiry before/after I/O, endpoint/credential skips, failed peer creation,
and context commit. Keep dispatch accounting and exactly-once lease settlement;
cancellation retains existing RAII cleanup and conservative Redis TTL behavior,
not a new promise of immediate remote release.

Measure projection work separately from Redis I/O for no-limit, limited, rejected
and dependency-failure attempts. Compare copied bytes, allocations, Arc operations,
retained owners and async-frame costs before choosing an API. Do not infer a
throughput or memory improvement from fewer DTO fields alone. Freeze scope and
obtain independent IMPL plan/final review before implementation.

## Original knowledge and verification

- [Prepared admission ownership](../../skills/04-review/ai-routing/regression-guards/prepared-attempt-owns-admission-input.md)
- [Runtime failover boundaries](../../skills/04-review/ai-routing/regression-guards/ai-failover-tests-must-cross-runtime-boundaries.md)
- [Predispatch deadlines](../../skills/04-review/ai-routing/regression-guards/ai-predispatch-scan-rechecks-deadlines.md)
- [Existing limiter borrow](INDEX.md#credential-reservation-limiter-borrowing)

Fresh unchanged-source verification passed 36 dispatch, 25 attempt-runtime,
seven HTTP-attempt and 104 upstream-peer tests (172 total). These verify existing
behavior, not a new optional/borrowed input contract or live Redis cancellation.
No prototype, benchmark, full suite or live integration ran for this audit.
