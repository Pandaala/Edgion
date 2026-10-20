# Decide ExtProc terminal stream retirement scope

Status: deferred for user decision. Scope: synchronous AttachedExtProc terminal
transitions, not normal successful half-close.

## Evidence

`edgion-gateway/src/plugins/http/ext_proc/attached.rs` replaces the state with
Finished in `close`, `mark_terminal` and `release_inflight` while holding its
parking_lot state mutex. `terminalize_for_logging` performs this replacement
only for Lazy/Open, leaving Failed unchanged. Replacing Open drops its stream
before the state guard is released.

`ExtProcStream` in `client.rs` has no custom Drop implementation. Its fields own
the optional outbound sender, an inbound opening future or streaming response,
the timeout value and the target's OwnedSemaphorePermit. Field destruction and
permit release therefore occur within the owner's state critical section at
these sites. This does not establish a network wait, reachable deadlock or
measured contention.

The shared owner is request-scoped. Response callback cancellation invokes
`release_inflight` while the response driver's async guard is still held.
Moving stream retirement outside the owner state mutex would not move it
outside every enclosing lock or eliminate its total destruction cost.

## Decision

Retain the current critical section, or investigate replacing state under
the mutex and dropping the retired stream after releasing that guard. The latter
could shorten owner-lock occupancy when an open stream is present, but it also
makes Finished observable before all old stream resources have been released.
Current assignment keeps other state-lock users excluded during that release.
The value of changing this ordering needs evidence, not just a shorter lexical
lock scope.

The exchange and `finish_success` paths already move the stream out before an
await, leaving Failed as a cancellation sentinel. They retain different protocol
and failure contracts and are not proof that synchronous terminal retirement
can change ordering without review. No second state machine, detached cleanup
task or generic retirement queue is selected here.

## Required acceptance

- Preserve all method-specific states and summaries: close changes only pending
  summary text; mark_terminal overwrites outcome/reason; release_inflight changes
  no summary; logging preserves Failed and already settled summaries.
- Establish the permitted ordering of state publication, sender/inbound release,
  permit availability and subsequent summary updates for every changed caller.
  Resources must still be released before the terminal method returns.
- Account for response driver body/trailer cancellation guards and request-body
  owner paths. Do not introduce replay, reopen a finished owner, weaken the Failed
  sentinel or let an in-flight exchange restore a retired transaction.
- Preserve actual clean half-close and matching-verdict/EOF behavior in the
  successful async path. Dropping a stream is not a substitute for that protocol.
  Keep deadlines, fail-open/fail-close policy, terminal diagnostics and the
  once-only summary claim.
- Test Lazy/Open/Finished/Failed, repeated calls, pending/settled/emitted summaries,
  real sender closure and permit return. Add focused state/resource-order probes
  before claiming that the new ordering is safe; existing serialized state tests
  alone do not establish concurrency equivalence.
- Measure owner-lock occupancy and total cleanup separately under representative
  terminal paths. Include outer driver lock ownership and demonstrate whether
  another legitimate observer actually contends. Do not infer throughput or
  memory gains from moving a drop statement.

This differs from [log-05](log-05-attached-summary-rendering.md), which concerns
formatted summary storage, and [body-03](body-03-mutation-snapshot-retirement.md),
which concerns request-body generation retirement. Original protocol constraints:
`skills/04-review/ext-proc/regression-guards/ext-proc-interactive-continue-and-replace.md`
and `skills/04-review/plugin-runtime/regression-guards/response-callback-downstream-cancellation.md`.
Use IMPL after user selection. This task changes no runtime code and contains
no new contention, destruction-order or live transport experiment.
