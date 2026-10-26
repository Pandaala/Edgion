# Evaluate credential half-open probe ownership

Status: deferred representation decision for user review; retain current code.
Inspected baseline: `c095d8458`.

## Evidence and value

`ai/runtime/attempt_runtime.rs::admit` moves AiCredentialOutlierHandle into
OutlierLease. That handle owns the captured backend policy generation and one
PassiveEjectionState Arc. Only successful half-open admission clones the state
Arc into HalfOpenProbe; Closed and Rejected admission do not make this clone.
The probe retains its generation and completion flag so verdict and Drop can
finish or release the exclusive probe safely. This is one reference-count
increment/decrement, not a copied state payload or a new allocation.

The credential handle also serves apply_retry_after/apply_cooldown, including
when the optional probe has been consumed. Those operations require the backend
policy's cooldown cap and the credential state. A verdict consumes the probe
when present; otherwise it uses the handle. The runtime's verdict_recorded latch
prevents a second normal verdict after the first completion. Simply moving the
handle's state into the probe would leave the remaining handle incomplete.
A borrowed probe inside the same movable lease would introduce a self-reference;
do not replace safe ownership with an unsafe pointer shortcut.

This differs from the breaker permit transfer: its input owner was no longer
needed once the permit existed. It also differs from
[Service completion tokens](outlier-01-completion-token-ownership.md), which concern
Service lookup and retired-generation retention. Current production searches find
the concrete HalfOpenProbe consumer in AI attempt runtime, not Service selection.
PassiveEjectionState is the shared state-machine owner; sharing that kernel does
not establish that Service uses this probe API.

## Decision

Prefer retaining the current representation unless a measured half-open workload
justifies changing the canonical ownership interface. An alternative could make
one admitted credential lease own the state and optional generation-bound probe
proof, or expose a reviewed state-access/owner-return boundary on the canonical
probe. These are design candidates, not an approved implementation. Keep policy
ownership, post-verdict cooldown access and cancellation release explicit.
Do not add a second ejection state machine, global registry or compatibility shim.

The possible saving applies only to half-open admissions, while a representation
change can affect closed requests, lease size, Drop logic and every failure path.
Measure those costs separately; no net CPU, heap or throughput gain is established.

## Acceptance for a selected design

- Freeze an exact implementation allow-list and independently review the owner
  and transition API. Keep the captured policy/state generation through alias
  removal, policy replacement and fallback successor publication.
- Preserve one exclusive probe, CAS/revalidation and lock ordering, generation
  checks, failure backoff, cancellation without verdict and stale-probe rejection.
  A stale verdict must not close a newer explicit provider cooldown.
  Preserve threshold-only failure arbitration and ordinary Service recovery.
- Preserve cooldown parsing, zero handling, min/max caps and explicit-cooldown
  ownership after verdict. Do not conflate normal success with probe success.
- Preserve runtime verdict-once behavior and real-EOS credential-success gates,
  observation-off health recovery, gateway termination and provider failures.
  Admission order remains breaker, outlier then reservation; Redis rejection,
  cancellation and predispatch teardown release the existing local owners.
- Exercise Closed/Rejected/Probe paths, success/failure/drop, later cooldown,
  policy changes, expired cooldown races and overlapping old/new generations.
  Compare owner counts and lease layout across all paths; measure probe-heavy and
  ordinary traffic separately without adding request tracing or metrics.
- Keep existing tests and integration fixtures. Require independent IMPL plan
  and final review plus project checks before any production change.

Original knowledge: [runtime failover boundaries](../../skills/04-review/ai-routing/regression-guards/ai-failover-tests-must-cross-runtime-boundaries.md)
and [prepared admission ownership](../../skills/04-review/ai-routing/regression-guards/prepared-attempt-owns-admission-input.md).
Canonical sources: `ai/runtime/credential_outlier.rs`, `ai/runtime/attempt_runtime.rs`
and `backends/outlier_detection/state.rs` under `edgion-gateway/src/`.

Fresh unchanged-source verification passed 17 credential-outlier, eight passive
state, 26 attempt-runtime and 39 dispatch tests (90 total). These establish the
current baseline, not a redesigned lease or a measured performance improvement.
No prototype, full workspace test suite, live integration or benchmark ran.
Formatting, workspace all-target check/Clippy, agent-doc validation and diff
hygiene passed with existing unrelated Clippy warnings.
