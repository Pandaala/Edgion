# CtxSet extraction and trim projection ownership

Status: deferred for user decision. No runtime change.
Source baseline: `d3810fb86`.

## Evidence and scope

CtxSet::process_rule starts with a Cow over static/source text or an owned template
result. apply_extract copies the selected regex capture into a String, and the
trim transform copies value.trim() into another String. Optional mapping can
discard those intermediate values immediately. Without mapping, process_rule
returns the resulting String to run_request, whose canonical set_ctx_var adapter
copies key and value into independently owned context storage.

Borrowing capture/trim slices could avoid intermediate selected-text copies,
especially before a mapping result replaces them. Empty selected text need not
allocate, so this is not a fixed per-operation allocation claim. Regex capture
execution, template rendering, mapping and final context storage retain their own
costs; no throughput measurement has been performed.

This is separate from ctx-01's regex replacement-result transfer and spare-capacity
tradeoff. It does not authorize changing the canonical context setter, other
transform algorithms or template semantics.

## Decision requested

Choose scoped borrowing, an explicit owner-plus-range representation, or retain
the current compact copies after comparing real workloads. Borrowed static/header
inputs and owned template/key-source inputs need different lifetime treatment.
Assigning a slice borrowed from an owned Cow back over that same Cow cannot safely
preserve its owner. Do not solve this with unsafe extensions or self-references.

Separate source and projection locals can retain an owned large input through
later transform/mapping work. Current extraction can release the original owned
input after copying a small capture; a borrowed projection would extend that
input's lifetime. This alone proves neither higher peak memory nor lower total
allocation: the current copy overlaps source and result while constructing it.
Measure release points and simultaneous live buffers, not just final capacities.

Keep sequential rules: publish each completed value before evaluating the next
rule, so later ctx reads observe earlier writes. Do not batch outputs or retain
session loans across the mutable setter. A future setter-ownership change needs
its own review; this proposal grants no permission to move large backing buffers
into long-lived context values.

## Acceptance before implementation

Use IMPL with an exact allow-list and independent plan/final review. Preserve
value/template/from priority, missing-source fallback, extraction group selection,
nonmatching/absent optional captures versus present-empty captures, and the current
compiled-regex path. Keep transform admission at zero or one operation and retain
defensive priority for bypassed validation. Unicode trim semantics, untouched
whitespace when trim is disabled, mapping hit/default/miss and rule defaults must
remain byte-equivalent.

Exercise borrowed and owned inputs, long input with short/empty capture, unchanged
and all-whitespace trim, lint-permitted Unicode text/whitespace, extraction followed
by trim, and mapping that replaces or rejects the intermediate. Compare final
context writes, cross-rule visibility, setter failures and PluginLog outcomes.
Preserve the existing template and setter policy: CtxSet currently has no template
output byte budget or setter value limit; do not invent one as an optimization.

Measure copied bytes, allocation calls and source/intermediate release points
separately from the final setter copy. Include missing/default and no-mapping
paths, not only mapping hits. No schema field, new cache, tracing, metric or
global transform abstraction is implied by this task.

## Sources and verification limits

- Gateway `plugins/http/ctx_set/plugin.rs`: process_rule, apply_extract,
  apply_transform and run_request.
- Gateway `plugins/runtime/session_adapter.rs`: set_ctx_var.
- `skills/04-review/ctx-set/regression-guards/one-transform-operation.md`.
- `skills/04-review/ctx-set/review-evidence/ctx-set-design-decisions.md`.
- `skills/04-review/plugin-runtime/regression-guards/request-template-policy-unification.md`.
- Existing deferred task `ctx-01-regex-replacement-storage.md`.

Fresh unchanged-code verification passed all 12 Gateway CtxSet tests. This
validates the current baseline, not a proposed projection or performance result.
