# Evaluate validation-only raw JSON traversal

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Inspected baseline: `3b5641e87`.

## Evidence and decision

edgion-dsl::json::parse_raw always calls raw_node with root retention enabled.
A top-level object therefore owns a member Vec and decoded member-name Strings,
separate from its duplicate-detection set. Gateway JsonBodyScanner::finish,
JsonRead on strings and JsonSetBody discard that RawDocument. The leaf set_member
also discards documents from raw-value admission and final-output validation.
Getters and reconstruction of the original object do need its member view.

A validation-only entry reusing the same visitor could omit discarded top-level
member storage. Simply switching retain to false, however, also skips reserve_vec
work charges. Those charges are non-refundable and can determine whether later
DSL operations succeed. This is not equivalent to the completed nested-key move,
which preserves every charge. Arrays/scalars do not have the same root-member
allocation opportunity. No workload benefit or allocation total has been measured.

Decide whether measurements justify a separate validation mode and how its work
ledger should behave. A preserving design needs reviewed accounting at the same
failure boundaries; a deliberately revised ledger needs explicit acceptance of
changed budget outcomes. Current reserve_vec charges depend on Vec capacity, and
try_reserve_exact does not promise an exact allocator capacity. A virtual growth
counter is not automatically an equivalent replacement. Removing allocations also
removes their possible allocation failures; state that limit of equivalence.

## Required boundaries

Reuse the canonical strict parser and policy, not another JSON implementation or
an AST/body cache. Preserve syntax-first errors, recursively decoded duplicate
keys, surrogate validation, depth/node/source limits, exact raw lexemes and the
initial/final copy accounting. Preserve malformed-versus-resource-error mapping.

Construction must still check the combined output's depth and nodes before
publication; separately valid inputs do not prove the combined shape is valid.
Retain source-type behavior, missing versus null, request-stage checks, physical
body limits, mutation freeze and one atomic body overwrite. No response-body
aggregation or new logging/metrics are authorized. Callers that consume members
must retain their current owner and ordering.

## Acceptance for a later implementation

Compare allocations, retained bytes and CPU for plain/escaped object names,
wide/nested objects, arrays/scalars and repeated calls. Exercise malformed trailing
input, decoded duplicates, surrogate errors, exact caps and combined-shape failures.
Test near-budget rejection at each former reservation boundary and subsequent DSL
operations using the same account. Document any intentionally changed ledger or
allocation-failure behavior rather than presenting it as invisible optimization.

Use IMPL with an exact Gateway/leaf allow-list and independent review. Fresh
verification of the current code passed nine DSL JSON and seven Gateway JSON
builtin tests. No candidate, allocation benchmark or live integration ran.
Canonical authority: skills/04-review/dsl/regression-guards/bounded-json-reconstruction.md
and skills/03-coding/dsl/builtin-error-semantics.md.
