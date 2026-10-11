---
id: ARCH-011
status: candidate
kind: behavior-defect
priority: P2
primary_partition: PART-241
independent_verification: confirmed
---

# Give AiGuard composition ownership of the final replacement budget

## Current contract and composition gap

AiGuardConfig.max_replacement_bytes is one inclusive aggregate replacement-text
ceiling. `ai/guards/request/rules.rs` bounds live local replacements against it;
`request/evaluator.rs` independently bounds external replacements against the
same value. `request/plugin.rs::run_request` combines these decisions through
merge, then encodes and publishes their replacements. The merge helper accepts
no budget and performs no final aggregate check.

With a ceiling of three decoded bytes, local replacement A="aaa" and external
replacement B="bbb" each pass their producer's check. The disjoint final set
contains six bytes and reaches edit encoding/publication when the separate
physical body cap permits it. Source tests cover individual producer limits but
not this union. This is a static call-chain conclusion, not an executed case.

The physical body cap and bounded producer allocations still apply. This is a
configured logical-limit violation, not evidence of unbounded storage or a
response-aggregation defect. Its ownership root is that no component validates
the final decision: partial proofs are treated as a proof of their composition.
Adding another producer or changing overrides would widen that gap.

## Bounded correction and alternatives

Make the request decision composer check the final aggregate before JSON edit
encoding or body publication. Retain external-overrides-local semantics, then
use checked arithmetic over final replacement values. Classify overflow and
above-cap totals through the existing dependency_failure policy. Same-key
overrides must replace the old charge; a smaller replacement releases budget.

Keep producer-local checks for early allocation/admission bounds and selected
key/whole-token validation before publication. Local masking runs before external
evaluation so unmasked selected text does not leave the gateway. Evaluator
response/replacement caps are independently necessary and remain unchanged.

A final borrowed checked traversal is the smallest correction, without another
text copy. Validate final keys against the selected spans before encoding;
once membership and uniqueness hold, the final set contains at most 512 items.
The current external parser alone does not establish that item-count bound:
invalid unique keys are presently rejected later, and its independent response
byte cap still applies. Incremental merge accounting can avoid the traversal but
adds bookkeeping. Changing only one producer's limit does not establish the
final invariant. No request/response lifecycle merger, new body cache, relaxed
strict envelope or RawWindow policy move is required.

## Invariants and future validation

Preserve decoded-byte inclusive limits, equal-key override behavior, existing
failOpen treatment of dependency failures, stricter rejection/generation errors,
post-local-mask evaluator input, and one atomic generation-checked edit batch
through the sole request BodyStorage. No prompt, replacement, endpoint or raw
dependency error may enter logs.

Future cases should cover disjoint producer results at the exact cap and cap
plus one, same-key shrink/expansion, empty replacements and invalid external keys.
Publication wiring must show no body mutation on failed composition under both
failure-policy settings. Test the pure composer directly and reserve mocks for
the publication connection; keep the required private unit-test layout.

## Original context and review status

The analyst read original AiGuard span/skip and ordering/publication decisions,
request-template policy ownership, native Responses restrictions, canonical SSE
architecture and window bounds. The schema specifies the aggregate ceiling;
operator documentation lists the same cap and its failure policy. Searches found
no decision or active task accepting separate local/external total budgets.
All eleven guard primaries and eight unit test files were fully read by the
analyst. Strict request/response wire envelopes and separate lifecycle owners
are legitimate and were not reported as duplication debt.

This root is separate from flat session capabilities, transport admission
metadata, compiled lookup keys and JSON syntax whitespace. Exact evidence and
independent decision bindings are in STATE.json and partition reports.
