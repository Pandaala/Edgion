---
name: rule-id-exclusion-scope
description: Review the accepted scope of rule-ID exclusions in native WAF policies.
knowledge_type: intentional-design
---

# Rule-ID exclusions in route-specific WAF policies

The maintainer closed waf-04 (P3) with the current scope accepted. A route
with a known false positive can use a dedicated WAF configuration whose rule
source applies `SecRuleUpdateTargetById` to the affected field. The exception
applies wherever that configuration runs. Other routes can keep the baseline.

`seclang/directive.rs` recognizes static removal and target updates by ID.
`parse_ctl_plan` in `seclang/semantic/actions.rs` rejects the two request-time
ID forms. The [transaction controls](../regression-guards/request-scoped-control-actions.md)
support request-time exclusions by tag, which can cover multiple rules.
Excluding a target suppresses its evaluation by that rule. A requirement
to retain matches for observation needs separate consideration.

This choice keeps the runtime control language smaller. Operators may have
to maintain separate configurations, and imported tuning rules that require
either ID control will fail strict compilation. Reconsider if deployments
require those rules verbatim or need exceptions based on transaction
conditions that route-specific configurations cannot express.

The closure was verified by source inspection. Runtime behavior was unchanged.
