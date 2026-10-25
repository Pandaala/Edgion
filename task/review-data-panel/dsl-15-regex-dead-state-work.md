# DSL streaming regex dead-state work

Status: deferred optimization and failure-visibility decision for the user.
Source baseline: `e7b761eb1`. No runtime change.

## Evidence

`StreamingRegexScanner::push` in Gateway `plugins/http/dsl/lang/body_scanners.rs`
advances the prepared dense DFA for every byte and stops only after a match state.
It does not inspect dead states. The pinned regex-automata 0.4.18 Automaton
contract makes dead states absorbing: they cannot lead to a future match.
Dead states are distinct from quit states and from ordinary nonmatching states.

A temporary dependency probe linked against the existing 0.4.18 build artifact
confirmed that the default dense builder for `^a`, started with the same empty
Input, enters a nonmatching dead state on `b`; all 256 byte transitions and the
EOI transition remain in that state. This is a concrete redundant-transition
example, not an integrated scanner test, allocation measurement or benchmark.

VM Matches uses this scanner for both Str and Body. The Str path supplies one
complete string and then finishes. The Body path reserves through DslBodyOps,
wraps the scanner in LossyTextScanner and traverses the canonical immutable
snapshot. A dead state currently returns Continue, so later chunks still undergo
storage reads, lossy decoding and DFA transitions. Finish preserves the latched
match and final end-of-input transition; an ordinary nonmatching state can still
match at EOI, for example with an end anchor.

Storage checks cancellation and remaining scan allowance before fetching each
subsequent chunk. This is not a full-body length preflight. It charges the fetched
raw chunk before acting on Stop, and the facade refunds unconsumed allowance
only after a successful scan outcome. Returning Stop at a dead state and finishing with false can
therefore change later storage/scan failures into a false predicate and leave
more budget available to subsequent DSL operations. The same change can alter
cancellation visibility and worker lifetime. It is not merely a faster boolean
comparison despite the DFA already knowing the mathematical answer.

## Decision

1. Retain the current scan and state-transition behavior.
2. Suppress only redundant DFA transitions once the state is truly dead, while
   retaining Continue. This can preserve Body reads, decoding, error visibility,
   cancellation and fetched-byte accounting. Compare a check per chunk versus
   checking transitions within a chunk; additional branches also affect live
   patterns. The Str and Body paths share the scanner but have different costs.
3. Stop Body traversal early on a dead state. This needs explicit approval of
   changed failure visibility and scan refunds, with defined semantics for later
   DSL operations. Do not smuggle it into option 2 or justify it solely from the
   absorbing-state contract.

No option is approved by this task. Do not conflate a quit state, no match yet,
end-anchored uncertainty or a pre-existing matched latch with dead-state failure.
Use the dependency's semantic state API rather than hardcoding state identifiers.

## Acceptance for a selected implementation

Freeze the affected source and test allow-list through IMPL review. Preserve
literal-pattern preparation, the bounded compiled-generation owner, independent
Gateway validation, the leaf/runtime boundary and all existing DFA admission
limits. Add no per-request regex compilation, matcher cache, body cache, response
aggregation or hot-path logs/metrics.

Verify unanchored and anchored patterns, empty patterns/inputs, EOI matches,
multibyte and malformed body input at chunk splits, mismatch location and long
trailing data. For transition-only suppression, prove per-push controls and
complete fetched-byte charges remain unchanged, including later read failures,
scan exhaustion, refund availability for another operation and cancellation.
For early traversal stop, explicitly specify and test each approved difference
instead of retaining only successful final-boolean cases.

Measure anchored long dead tails and still-live unanchored searches separately,
with String, memory and spilled Body inputs and different chunk sizes. Separate
DFA transitions from lossy decoding, raw reads and scan-worker occupancy. A
reduced transition count alone does not prove lower request latency or throughput.
Existing integration fixtures remain unchanged unless separately authorized.

## Original sources and limits

- `edgion-gateway/src/plugins/http/dsl/lang/body_scanners.rs`: scanner state machine.
- `edgion-gateway/src/plugins/http/dsl/lang/vm.rs`: prepared cache and Matches paths.
- `edgion-gateway/src/plugins/http/dsl/lang/body_ops.rs` and `body_scan_budget.rs`:
  reservation, settlement and failure mapping.
- `edgion-gateway/src/request_body/operation.rs` and `storage.rs`: lossy adapter,
  scan control, fetched-byte accounting and worker ownership.
- [DSL security model](../../skills/01-architecture/02-gateway/dsl-engine-security-model.md)
- [Storage I/O ownership](../../skills/04-review/http-body/regression-guards/request-body-storage-io-deadline.md)
- Pinned regex-automata 0.4.18 `dfa/automaton.rs`: is_dead_state and is_quit_state.

This is separate from literal Body substring-pattern preparation in
[dsl-01](dsl-01-string-consumer-ownership.md). The regex DFA is already prepared;
this candidate concerns its execution after a terminal state, not compilation.
