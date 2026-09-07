---
name: add-from-builders-duplication
description: Use when reviewing plugins-runtime `plugin_runtime.rs` "four `add_from_*` builder functions repeat ~315 lines of identical scaffolding / extract a generic `add_plugins_inner<E, CF, AF>` helper with closures / add unit tests for the two untested builders (F-003)" findings. The wholesale extract-helper refactor is a deliberately declined readability-only consolidation.
knowledge_type: intentional-design
---

# plugin_runtime `add_from_*` Builder Duplication — Extract-Helper — DESIGN-TRADEOFF

**Audit date**: 2026-05-26
**Linked issue**: plugins-runtime F-003 (Low / Redundancy) — closed, not refactored.

## Conclusion

The four builder functions in `edgion-gateway/src/plugins/runtime/plugin_runtime.rs` —
`add_from_request_filters` (382–455), `add_from_upstream_response_filters` (463–539),
`add_from_upstream_responses` (547–613), `add_from_upstream_response_body_filters`
(621–697) — share the same loop skeleton (`is_enabled` → `validation_error` →
`validate_alias` → `create_*_from_edgion` → match `CreateResult::Ok(Some) | Ok(None) |
WrongStage` with push-to-`invalid_plugins` in both non-`Ok(Some)` arms). The finding
itself reports **"Maintenance burden only; no correctness risk"** — this is a
readability/consolidation concern, not an observable bug. The proposed extraction into
a generic `fn add_plugins_inner<E, CF, AF>(entries, stage_label, create_fn, add_fn,
&mut errors)` (with an extra `on_enabled` callback to host the CORS-config caching
specific to the request-filter variant) is **not accepted**.

Fix suggestions of the form "extract a generic `add_plugins_inner` helper that takes
the per-stage error prefix and the `create_*` / `add_*` / `ConditionalWrapper`
constructors as closures, then collapse each public function to a 1–2 line wrapper"
are **not accepted**.

## Core Rationale

**1. No runtime bug — purely hypothetical future-replication risk**

The Impact section concedes there is no defect today. The argued cost is that a
future logic change to the shared loop (alias-drop semantics, `Ok(None)` handling)
must be replicated across four sites. That is speculative maintenance, not an
observed regression. The project rule in
[style-consolidation-thresholds.md](../../review-method/intentional-design/style-consolidation-thresholds.md) requires
"real maintenance cost or reading cost evidence" before acting on a "looks
duplicated" finding; the four arms in question have been stable since the fail-closed
contract was introduced — no historical drift between them has been recorded.

**2. The CORS callback is direct evidence of a leaky abstraction**

The reviewer's own proposal includes an `on_enabled` callback that fires only for
the request-filter variant (to populate `self.cors_config` at
`plugin_runtime.rs:402–406`). The other three call sites would have to pass a
no-op. A "generic helper" whose signature carries a per-stage side-effect hook is
not a deduplication win — it shifts the per-stage divergence from a readable inline
block to an opaque closure-parameter contract. Readers of the upstream-stage
builders would have to scan the helper signature, confirm the closure they pass is
truly a no-op, and verify the helper invokes it at the correct point in the
sequence (between `validation_error` and `validate_alias`, before `create_*`).
That is more cognitive load than the current "skim 70 lines of obviously parallel
code" layout. Precedent (d) in
[size-and-similarity-heuristics.md](../../review-method/intentional-design/size-and-similarity-heuristics.md) declined the
same shape of refactor between `rate_limit` and `rate_limit_redis` for the same
reason: forcibly coupling two currently independent stages through a callback-laden
helper is negative-value when the per-stage divergence is the interesting bit.

**3. Signature bloat dominates the body — same antipattern as the giant-match precedent**

The single-arm body that varies across the four functions is the error-prefix
literal, the `create_*_from_edgion` call, and the `ConditionalWrapper::with_alias`
+ `add_*` pair — collectively ~15 distinct lines per function out of ~70. A
generic `add_plugins_inner<E, CF, AF>` would carry **three or four type
parameters** (entry, create-fn, add-fn — plus a filter-trait param if the trait
is not threaded as an associated type of `E`), **two or three closure trait
bounds** (`Fn(&E::Plugin, &str) -> CreateResult<...>`, `Fn(&mut Self, Box<dyn
Trait>)`, plus the optional `on_enabled` for the CORS-cache stage), and a
stage-label `&'static str`. The helper signature
alone is larger than each per-arm body it replaces. This is the same
"signature-bloat-dominates-body" antipattern documented in
[`dsl-builtins/giant-match-extract-fn-declined.md`](../../dsl/intentional-design/giant-match-extract-fn-declined.md):
extracting the body of a tight, parallel match into a generic wrapper makes the
caller-side prose longer, not shorter, and the type-parameter dance obscures the
fail-closed semantics that are the only thing a reader actually cares about in this
hot section.

**4. The path is init-only and security-sensitive**

These builders run only when an `EdgionPlugins` resource is built or reloaded
(ConfHandler path), not on the request path — so there is no performance angle.
What the path **is** sensitive to is correct `invalid_plugins` aggregation: any
mistake in error accumulation or push ordering propagates into
`seal_fail_closed`, which turns a non-empty `invalid_plugins` into a fail-closed
500. The current four-copy layout makes every push-to-`invalid_plugins` call
literally visible at its decision point; a closure-bearing helper would route them
through trait-object indirection and increase the surface area where a refactor
mistake (closure capturing the wrong `&mut`, mis-threaded `errors` vector) could
silently change the fail-closed contract.

**5. The "add unit tests" half of the proposal is independent and can be done without the refactor**

The finding bundles two asks: (a) extract the generic helper and (b) add unit
tests for `add_from_upstream_response_filters` and `add_from_upstream_responses`.
(b) does not require (a) — if test coverage of those two builders is genuinely
desired, file a separate narrow test-only task. Conflating them under "Redundancy"
inflates the change scope without justification.

## Fix Suggestions Not Accepted

- "Extract `fn add_plugins_inner<E, CF, AF>(entries, stage_label, create_fn, add_fn, &mut errors)` and collapse the four public functions to wrappers" — signature bloat + CORS callback leak; declined.
- "Add an `on_enabled` callback to the helper to keep the CORS-config caching out of the generic path" — promotes per-stage divergence into an opaque closure contract; readability regression.
- "Split into four files by stage to make the duplication invisible" — form-only relocation; does not reduce the line count or change the maintenance shape.
- "Introduce a trait `AddFromEntry` with associated types so each stage implements it" — disperses the fail-closed bookkeeping across four trait impls; same anti-pattern declined in [`plugin-runtime-god-match.md`](plugin-runtime-god-match.md) §2.

## Re-evaluation Triggers

Re-open this decision only if:

- A real bug is found that exists in one of the four builders but not the others (genuine drift, not "could drift") — at that point, fixing the divergence is the priority and the helper extraction may be justified to lock the contract in one place.
- A fifth pipeline stage is added that would force a fifth ~70-line copy — at that point the duplication threshold flips, and the extraction (with the CORS callback) becomes proportionate.
- The fail-closed semantics of `invalid_plugins` / `seal_fail_closed` are revisited and the per-builder push-points need coordinated changes — extracting the loop becomes a side-benefit of that work, not a standalone refactor.

## Reference Cases

- Issue file: `tasks/plugins/plugins-runtime-F003-add-from-builder-functions-duplicated.md` (closed 2026-05-26, not refactored).
- Source: `edgion-gateway/src/plugins/runtime/plugin_runtime.rs:382-697`.
- Project-level "looks duplicated needs evidence" rule: [`style-consolidation-thresholds.md`](../../review-method/intentional-design/style-consolidation-thresholds.md).
- Three-precondition checklist for line-similarity refactors: [`size-and-similarity-heuristics.md`](../../review-method/intentional-design/size-and-similarity-heuristics.md) — precedent (d) (`rate_limit` vs `rate_limit_redis`) is the closest sibling.
- Signature-bloat-dominates-body antipattern: [`../dsl-builtins/giant-match-extract-fn-declined.md`](../../dsl/intentional-design/giant-match-extract-fn-declined.md).
- Same-file central-dispatch design tradeoff: [`plugin-runtime-god-match.md`](plugin-runtime-god-match.md).
