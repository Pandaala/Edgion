# WAF final transformation buffer transfer

Status: deferred ownership and retained-capacity decision for the user.
Source baseline: `796921efb`. No runtime or dependency change.

## Evidence

In transaction.rs::evaluate_single_input_with_transform_memo, the non-multiMatch
path obtains a Cow from transform.rs::apply_transformations_borrowed. A result that
remains in the input location borrows input; a scratch-backed final result moves
one scratch Vec into Cow::Owned. Byte-equal output can still be scratch-backed.
Evaluation then borrows either variant into evaluate_transformed_input. On a hit,
the ordinary matched_variable callback copies the value through owned_copy before
resolving the selected name and allocating its qualified name. Structured dynamic
matching in json_inspection_runtime.rs also copies matching bytes, but constructs
its name from an already resolved candidate. Static sealed results use handles.
SemanticLength uses local decimal bytes, not an owned transformation buffer.

The final owned buffer therefore offers a possible transfer opportunity after
successful predicate evaluation. A local scratch borrow cannot survive as a
MemberMatch or TX observer: matched values persist and chain branches clone state.
Transferring ownership differs from borrowing and does not eliminate later copies
needed by current and cumulative matched-variable state.

The current owned_copy requests capacity for the final length; allocator capacity
is not guaranteed exact. Transform scratch can retain capacity from an earlier,
larger output. Moving it may remove a payload copy and allocation while retaining
more memory for longer. MatchStorage::push accounts for the outer member vector,
not final matched-value heap capacity. member_match_bytes and TX retained charges
use payload lengths. MaterializedStages scratch-capacity callbacks are no-ops;
these counters must not be presented as measurement of the complete live heap.

## Decision

Keep the final copy, transfer owned final output, or use a measured capacity-aware
transfer policy. Measure allocation count, copied bytes, capacity versus length,
peak overlap and lifetime across large-to-small recipes, hits, misses and chains.
Define whether excess capacity is retained, trimmed or copied and how that choice
interacts with allocation failure. Adding physical admission is a separate behavior
change requiring explicit design; do not silently redefine logical limits.

Preserve full transformation validation before predicates, borrowed-memo eligibility,
canonical predicate/capture execution and the value/name resolution order. Removing
the value-copy allocation removes a failure opportunity; record that consequence.
Keep ordinary and structured dynamic name resolution distinct. Do not expand the
proposal into static sealed-handle storage or SemanticLength, and do not assume
all returned values own transferable storage.

This differs from [multiMatch stage ownership](waf-07-multimatch-stage-ownership.md):
that proposal concerns retaining intermediate stages and early predicate ordering.
This proposal concerns transfer of one already validated non-multiMatch final value.

## Acceptance for an approved implementation

Compare against the original copy path for unchanged/changed/empty final output,
expansion followed by contraction, hits and misses, captures, negation, name
resolution failures, and both evaluation modes. Verify ordered errors at exact
and one-below operator, capture, TX and retained limits without claiming those
limits measure all payload capacity. Preserve matched-variable macros, cumulative
observers, chain rollback, later-phase reads and cleanup after every failure.

Measure retained capacity through member collection and TX commit, not only the
transform function. Check borrowed inputs remain valid and owned buffers have one
owner. Keep logical work and length charges unchanged unless explicitly approved.
No new body cache, response aggregation, request tracing or payload logs are needed.
Gateway remains the anchor; future shared-leaf edits need a frozen allowlist and
independent IMPL plan/final reviews. Do not modify integration fixtures.

## Knowledge and verification limits

See [fused JSON inspection](../../skills/04-review/waf/regression-guards/fused-json-inspection.md)
for dynamic owned values, sealed handles, transformation scratch, TX observations
and rollback. Current tests establish the current contract, not a proposed transfer
policy or measured performance gain. This task grants no implementation approval.
