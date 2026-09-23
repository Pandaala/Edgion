---
name: window-emitted-context
description: Review RawWindow generation transitions, semantic replacement, and terminal overlap ownership.
knowledge_type: regression-guards
---

# Window context follows emitted text

## Conclusion and scope

`RawWindow` context is the bounded UTF-8-safe suffix of text handed off by
successful `WindowOutput` decisions. It does not attest to transport delivery.
The canonical authority is the gateway SSE substrate contract in
`skills/01-architecture/02-gateway/00-overview.md` and AGENTS.md's HTTP body
handling invariants.

Release appends decoded current text to prior context. Semantic Replace appends
only replacement text represented by emitted edits. Discard preserves prior
context. Opaque Replace and raw-only Release across a semantically unavailable
span reset context. Every terminal decision skips next-generation overlap.
Per-bucket limits, deterministic global max-min fairness, and UTF-8 boundaries
apply to the concatenated logical text without building a full concatenation.

## Rationale and evidence

`edgion-gateway/src/sse/window.rs` previously derived replacement overlap from
replacement text alone, cleared history on Discard, and derived opaque replacement
context from original text that was never emitted. A context-only bucket could
also install nonempty replacement text despite contributing no wire edit.

`prepare_overlaps`, `replacement_texts`, `apply_decision`, and
`apply_semantic_replace` now implement the decision-specific lifecycle. Nonempty
replacement of a bucket without current fragments is rejected atomically. Its
empty replacement preserves prior context. No new Guardrail wire action or
bucket-membership policy is introduced.

## Alternatives and limits

Guardrail Reject terminates the stream; Discard-and-continue and opaque Replace
are substrate API cases rather than current Guardrail continuation paths.
Guardrail uses semantic Replace and can observe the multi-generation context
defect directly. Its existing callback rollback and terminal policy still own
any later disposal of output. This rule does not add response aggregation or
change Pingora/SSE framing.

## Verification and re-evaluation

Focused regressions cover three-generation transitions, UTF-8 global fairness,
context-only buckets, unknown raw spans, and final Guardrail replacement preview.
Terminal capacity tests additionally verify that no discarded overlap is charged.
Verified on 2026-09-23: the gateway library filters `sse::` (74 tests) and
`plugins::http::guardrail::response::` (18 tests) passed. Workspace all-target
check and Clippy, formatting, agent-doc validation, SSA force, metrics inventory,
Pingora source policy, and Gateway API transform contract checks passed.
Independent read-only code and knowledge review returned LGTM.
Re-evaluate on any new decision kind, consumer continuation policy, or bucket
membership change.

## Resolution references

- `sse-window-strict-residency-and-continuity` — corrected and verified on
  2026-09-23; independent review: LGTM.
