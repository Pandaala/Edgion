---
name: review-waf
description: Review knowledge for the native WAF engine and its SecLang operators.
---

# WAF review knowledge

Read [routing and classification](../knowledge-routing.md) before changing a rule.
Check the current compiler, transaction, and detector code before applying a past conclusion.

## Regression guards

- [Static tags select validated whole units before compilation](regression-guards/static-tag-selection-before-compilation.md)

- [JSON inspection uses one grammar pass and bounded selected retention](regression-guards/fused-json-inspection.md)
- [Native XML inspection preserves bounded parsing and ordered effects](regression-guards/native-xml-inspection.md)
- [Multipart boundary discovery stays linear](regression-guards/multipart-boundary-search-bounded.md)
- [Native injection operators use bounded detection and transaction semantics](regression-guards/native-injection-operators-use-bounded-detection.md)
- [Selected WAF control actions are request scoped](regression-guards/request-scoped-control-actions.md)
- [Named WAF profiles publish one validated generation](regression-guards/named-profile-selection-and-bundle-publication.md)

## Intentional design

- [Retain bounded geometric growth for JSON token scratch](intentional-design/json-token-geometric-growth.md)

## Reviewed optimization design

The historical optimization specification
(`tasks/pending/waf-optimization-design/01-spec.md`) and its serial plan described
candidate work; those task records are absent from the current tree. The regression guards above record
current implementation boundaries; design approval alone does not establish
implementation or measured adoption. The fused JSON guard records the narrower
contiguous-input implementation and its verification limits.

## Remaining standalone tasks

- [Trusted language policy profiles](../../../tasks/todo/waf-language-policy-profiles.md)
- Representative Gateway performance, Kubernetes admission/reload and x86-64
  verification require fresh evidence via the
  [testing guide](../../05-testing/01-integration-testing.md). Their historical
  task records are absent; removal does not establish completed validation.
- [Shared request-body inspection](../../../tasks/todo/waf-shared-request-body-inspection.md)
- [Native SIMD phrase search](../../../tasks/todo/waf-native-simd-search.md)
- [Native DFA state layout](../../../tasks/todo/waf-dfa-state-layout.md)

Conditional optimization tasks require measured evidence before implementation;
they are not unfinished acceptance gates for the delivered request WAF.
