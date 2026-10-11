---
id: ARCH-227
status: confirmed
kind: architecture-debt
priority: P3
partition: PART-169
analyst: /root/analyze_conversion
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---
# ARCH-227: Commit only surviving transformation indices in the semantic fold

## Current ownership and concrete cost

`edgion-waf/src/seclang/semantic.rs::fold_local_rules` first inspects every rule/action envelope without an output sink. It computes the number of effective transformations after the last `t:none`, charges that final count, and fallibly reserves exactly that many index slots (`5809–5946`). Its second inspection supplies the retained `effective_transformations` vector as a sink (`5953–6005`).

The sink path in `inspect_fold` nevertheless pushes each transformation immediately. On `t:none` it truncates back to the current fold's start (`6103–6135`). Truncation removes indices but leaves any expanded Vec capacity. The output owner therefore temporarily materializes, and can retain capacity for, a prefix that the count/reserve pass deliberately excluded.

For example, a valid SecRule with `id:1,phase:2,pass`, 128 repeated `t:lowercase` actions, and a final `t:none` has zero effective transformations. Its count pass reserves zero index slots. The commit pass pushes 128 indices and then truncates to zero; the nonempty allocation survives in the draft's private vector. This source shape is within the current default 1,024-action per-list bound. The mechanism also occurs when a reset prefix grows beyond the capacity reserved for other folds' surviving transformations.

The draft is retained by `ValidatedSemanticDocument` and then by the compiler's input owner. Production `WafConfig` uses default frontend limits for strict bundle compilation, including policy-selection compilation. `include.rs:1035–1044,1195–1226` creates and validates this draft; `compiler.rs:2550–2580` retains it while preparing compiler inputs. Only surviving indices reach the compiler's effective-transformation iteration at `compiler.rs:5082–5090`.

This is bounded cold frontend debt. Default syntax limits bound each discarded prefix; the excess capacity is a high-water mark, not an allocation added for every reset. It is not a request-path allocation, an executable-generation payload leak, or a demonstrated violation of a global heap ceiling. Existing semantic charged-byte values are logical projections and must not be relabeled as measured allocator usage. The concrete defect in ownership is doing discard-only index materialization after a final-count reservation, including ordinary Vec growth outside that reservation.

## Bounded improvement and alternative

Keep complete action classification and fold inspection. Change only construction of the private effective-index output so it appends transformations strictly after the final `t:none` for each fold. Determine that final suffix from the already classified immutable action records, or carry the inspection's private final reset boundary into commit. Inspect all original actions in their existing order; suppress only output-index writes for the discarded prefix.

All committed indices then belong to the final sequence for which `fold_local_rules` already calculated charges and reserved storage. The improvement removes reset-discarded index writes and their capacity growth while preserving the two-pass validation/admission boundary. It needs no public type, new compiler representation, cache, workspace dependency, or generic folding framework.

An alternative is to admit discarded-prefix capacity explicitly, grow it fallibly and later compact it. That introduces another accounting/allocation lifetime for data that no consumer requires. Charging every original transformation as effective would change the language's accepted reset and logical-limit semantics and is outside this proposal.

## Preserved contracts and verification

Every original action must still receive syntax, operand, macro, transformation and fold validation, including invalid or forbidden actions before a later `t:none`. Preserve duplicate/forbidden/missing-ID/missing-disruptive/status-pass precedence; the later content-error-before-aggregate-excess rule; absent versus present-empty lists; chains and cross-document adjacency; logging overrides; final reset identity; repeated reset behavior; and exact effective transformation order.

Keep semantic owned/retained logical charges, final effective limits, final-count fallible reservation and typed allocation errors. Do not add a new rejection for an otherwise valid discarded prefix, raise limits, prune source records, skip full-original profile validation, or change body modes, capture/TX effects, runtime transformation checkpoints or request BodyStorage ownership. This recommendation does not attempt to preserve an allocation failure originating solely from the removed unnecessary growth.

Actual `semantic_test.rs` bodies read include canonical operator case policy, normalized folds, error precedence, exact/one-less fold limits, the exhaustive five-action suffix model, and exact logical owned/retained boundaries. The suffix model checks effective values rather than reset-induced vector capacity. Future focused verification should cover a large final-empty reset prefix, a small surviving suffix after several resets, preceding/later folds with surviving indices, and a malformed pre-reset action. Preserve the existing suffix model and error/limit tests. A private zero-effective owner can demonstrate absence of an index allocation without assuming every allocator returns exactly the requested nonzero capacity.

## Original decisions and limits

The original static-tag selection guard requires validation of the complete original baseline before selection and explicitly limits frontend transients independently of compiler peak claims. Named profile publication preserves cold strict compilation and immutable request generations. Those accepted boundaries remain intact. The WAF optimization specification's parser scratch, SIMD and shared-scheduling work does not own this semantic reset output seam. Current review artifacts contain no existing `fold_local_rules`/effective-reset owner finding; the concurrently reviewed compiler occurrence-versus-unique-capacity issue concerns another vector and another preparation boundary.

No implementation, tests, benchmarks, external oracle or live traffic were executed. This finding establishes unnecessary index materialization and reserve/commit mismatch by current source. It claims neither an incident nor a numerical performance/RSS benefit, and it does not expand semantic logical budgets into a global allocator-bound guarantee.
