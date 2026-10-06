---
name: review-waf
description: Review knowledge for the native WAF engine and its SecLang operators.
---

# WAF review knowledge

Read [routing and classification](../knowledge-routing.md) before changing a rule.
Check the current compiler, transaction, and detector code before applying a past conclusion.

## Regression guards

- [JSON inspection uses one grammar pass and bounded selected retention](regression-guards/fused-json-inspection.md)
- [Native XML inspection preserves bounded parsing and ordered effects](regression-guards/native-xml-inspection.md)
- [Multipart boundary discovery stays linear](regression-guards/multipart-boundary-search-bounded.md)
- [Native injection operators use bounded detection and transaction semantics](regression-guards/native-injection-operators-use-bounded-detection.md)
- [Selected WAF control actions are request scoped](regression-guards/request-scoped-control-actions.md)
- [Named WAF profiles publish one validated generation](regression-guards/named-profile-selection-and-bundle-publication.md)
