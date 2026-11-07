---
name: review-waf
description: Review knowledge for the native WAF engine and its SecLang operators.
---

# WAF review knowledge

Read [routing and classification](../knowledge-routing.md) before changing a rule.
Check the current compiler, transaction, and detector code before applying a past conclusion.

## Regression guards

- [Semantic folds commit only surviving transformation indices](regression-guards/semantic-transformation-reset-output.md)

- [Form decoding borrows unchanged scratch](regression-guards/form-decoder-borrowed-scratch.md)

- [Static tags select validated whole units before compilation](regression-guards/static-tag-selection-before-compilation.md)

- [JSON inspection uses one grammar pass and bounded selected retention](regression-guards/fused-json-inspection.md)
- [Native XML inspection preserves bounded parsing and ordered effects](regression-guards/native-xml-inspection.md)
- [Multipart boundary discovery stays linear](regression-guards/multipart-boundary-search-bounded.md)
- [Native injection operators use bounded detection and transaction semantics](regression-guards/native-injection-operators-use-bounded-detection.md)
- [Marker-free backslash decoders stay borrowed](regression-guards/transform-marker-free-decoder-borrowing.md)
- [htmlEntityDecode follows the libmodsecurity reference boundaries](regression-guards/html-entity-decode-reference-semantics.md)
- [Regex scratch builds its VM cache lazily; capture programs decide without capture tracking](regression-guards/regex-boolean-scratch-lazy-vm-cache.md)
- [Chain ordinary work accumulates across members and branches](regression-guards/chain-ordinary-work-accumulates-per-unit.md)
- [Selected WAF control actions are request scoped](regression-guards/request-scoped-control-actions.md)
- [Rule units stage TX edits in place with an undo journal](regression-guards/tx-unit-staging-in-place.md)
- [Named WAF profiles publish one validated generation](regression-guards/named-profile-selection-and-bundle-publication.md)
- [Front-end dedupe, selector scans and nested quantifiers are charged before work](regression-guards/frontend-dedupe-and-selector-scans-are-charged.md)

## Intentional design

- [Retain bounded geometric growth for JSON token scratch](intentional-design/json-token-geometric-growth.md)

## Reviewed optimization design

The historical optimization specification
(`tasks/pending/waf-optimization-design/01-spec.md`) and its serial plan described
candidate work; those task records are absent from the current tree. The regression guards above record
current implementation boundaries; design approval alone does not establish
implementation or measured adoption. The fused JSON guard records the narrower
contiguous-input implementation and its verification limits.

## Untracked validation and optimization

The request-path optimizations formerly tracked as `tasks/todo/waf-06`
(borrowed marker-free decoders), `waf-15` (lazy boolean-regex VM cache) and
`waf-21` (static SQLi delimiter tables) were completed in the 2026-11-07
optimization pass and are recorded by the guards above. Open WAF work lives in
`tasks/improve-issues/waf-*.md` (note the unrelated open item that reuses the
`waf-06` id) and `tasks/todo/waf-streaming-raw-member-msg-action.md`. The
following were deliberately dropped, not completed: representative Gateway performance measurement, Kubernetes
admission/reload validation (including the audited language-selection
profiles), and x86-64 hardware verification. Do not cite them as passed; if
needed, gather fresh evidence via the
[testing guide](../../05-testing/01-integration-testing.md).

Speculative optimizations (shared raw/structured body inspection, native SIMD
phrase search, premultiplied DFA state identifiers) are likewise untracked.
Open one only when measured profiling attributes a material cost to that area;
none of these are acceptance gates for the delivered request WAF.
