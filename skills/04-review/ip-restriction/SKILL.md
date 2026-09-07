---
name: review-ip-restriction
description: Review knowledge for ip-restriction, grouped by decision type.
---

# ip-restriction review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Mandatory design

- [IpRestriction — group name appears in deny logs](mandatory-design/group-name-in-deny-log.md)

## Intentional design

- [Stream IP-restriction `init_failed` guard duplication — DESIGN-TRADEOFF](intentional-design/stream-init-failed-pattern-duplication-not-extracted.md)

## Regression guards

- [IpRestriction documentation uses named `IpGroup` entries](regression-guards/named-ip-group-documentation.md)
- [AccessDecision → StreamPluginResult Mapper Consolidated — FIXED](regression-guards/access-decision-to-stream-result-extracted.md)
- [Deny→allow→default IP evaluation algorithm — CONSOLIDATED](regression-guards/deny-allow-default-algorithm-extracted-to-evaluate-ip-access.md)
- [HTTP `IpRestriction::create` compiles matchers + fail-closed guard — FIXED](regression-guards/http-create-must-call-validate-and-init.md)
- [Stream-plugin controller preparse uses `PluginValidation` — FIXED](regression-guards/stream-preparse-uses-plugin-validation.md)

## Review evidence

- [IpSource::RemoteIp / KeyGet::RemoteIp — NOT-AN-ISSUE](review-evidence/ip-source-client-ip-false-positive.md)
