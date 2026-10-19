# Decide Guardrail held replacement-header storage

Status: deferred for user decision. Scope: semantic Guardrail precommit rejection.

## Evidence

`pg_response_head::response_head_commit_plan` prepares a rejection template after
final response processing when an active Guardrail claim is eligible for Hold.
`AttachedGuardrail::prepare_head_replacement` in
`edgion-gateway/src/ai/guards/semantic/attached.rs` builds a 403 ResponseHeader,
copies the first Server and Strict-Transport-Security values when present, sets
fixed JSON/no-store/body-length fields and reconciles managed response IDs. It
stores the completed header in a request-owned OnceLock. It does not clone the
complete origin header or retain origin entity metadata.

This preparation happens before the later policy outcome, including responses
that eventually pass. `head_replacement` clones the frozen header into a Box and
creates a vector containing static JSON bytes when replacement is requested.
`stage_head_policy_reject` in the sibling `response.rs` first checks the held
head and claims the response terminal owner, then requests this replacement.
Header construction/storage and rejection-time cloning are separate costs.

## Decision

Keep the ready-to-use template, or investigate freezing only the required owned
projection at the same post-onion boundary and materializing the header on reject.
The latter may reduce preparation for passing traffic but introduces a new
representation, delayed construction work and potentially different failure
handling. Snapshotting all request identity state or the whole response merely
to defer construction may retain more memory than the current small header.

The owner is shared and exposes its template through immutable references.
Moving the header out is not a drop-in replacement for cloning: any consuming
handoff must establish call multiplicity, ownership, synchronization, cancellation
and repeated-access behavior. Do not add a blocking lock or silently consume
first-wins state as a presumed allocation optimization.

## Required acceptance

- Preserve the exact snapshot boundary. Do not borrow original response fields
  across Hold or read later mutable session/context state when rejection occurs.
  Preserve the current first successful OnceLock publication behavior.
- Freeze the same first Server/HSTS values with opaque bytes and sensitivity
  metadata intact. Preserve managed correlation-ID values, custom echo and
  suppression, including their precedence over earlier template fields. Reuse
  canonical identity reconciliation; do not invent a second RequestId policy.
- Preserve 403, fixed JSON bytes and exact length, Content-Type and no-store.
  Do not carry origin body/entity headers or unrelated sticky cookies into the
  replacement. No response BodyStorage, spool or full-response aggregation is
  authorized; the existing semantic window and Hold limits remain authoritative.
- Preserve active-claim eligibility, native gRPC/AI conversion delegation, 101
  handling, sole final-head writer, terminal claim order and failed/missing
  replacement behavior. Keep deadlines, release/replace/fail outcomes, aggregate
  logging and cancellation cleanup. Construction moved after terminal claim
  needs explicit failure-policy review before implementation.
- Test pass/reject, repeated preparation/access, source mutation after capture,
  absent/present/opaque security fields, default-ID suppression and custom echo,
  terminal-owner conflict, replacement failure and cancellation. Existing
  attached-owner and pending-release regressions are starting points, not proof
  of a new representation's correctness.
- Measure pass/reject ratios, held lifetime, field/value sizes, custom-ID count,
  allocation calls and retained storage separately from rejection latency. Include
  concurrent held requests. Do not infer throughput or peak-memory improvement
  merely from postponing a HeaderMap allocation.

Related boundaries remain separate: ordinary pending-header execution ownership
in [response-01](response-01-pending-header-ownership.md), and generic outbound
integer-header storage in [outbound-01](outbound-01-content-length-integer-storage.md).
Original constraints: `skills/04-review/plugin-runtime/compatibility-constraints/filter-iteration-control-sync-phase-tradeoff.md`,
`skills/04-review/observability/intentional-design/request-trace-id-lifecycle.md`
and the repository's HTTP body handling invariants. Use IMPL after user selection.
This task changes no runtime code and provides no benchmark result.
