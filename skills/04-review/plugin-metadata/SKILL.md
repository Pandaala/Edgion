---
name: review-plugin-metadata
description: Review knowledge for plugin-metadata, grouped by decision type.
---

# plugin-metadata review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Mandatory design

- [ConditionSet and predicate ownership](mandatory-design/condition-set-single-owner.md)
- [JWT Claim Accessor Validation Belongs to Condition](mandatory-design/jwt-claim-accessor-validation-belongs-to-condition.md)
- [KeyGet Is Local, Synchronous, and I/O-Free — DESIGN-TRADEOFF](mandatory-design/keyget-is-local-io-free.md)
- [EdgionConfigData::KeyList uses KeyGroup (mirrors IpListData/IpGroup)](mandatory-design/keylist-group-structure.md)
- [MatchCore Payload Validation Belongs to Condition](mandatory-design/matchcore-payload-validation-belongs-to-condition.md)
- [SecretMatch Selects One Secret Data Key](mandatory-design/secretmatch-selects-one-secret-data-key.md)

## Regression guards

- [Condition descriptor type matches the schema discriminator — FIXED (cl-07)](regression-guards/condition-descriptor-type-matches-schema.md)
- [TimeRangeCondition RFC3339 preparse validation — FIXED](regression-guards/conditions-timerange-preparse-validation.md)
- [Disabled Plugin Entry Conditions Are Not Compiled/Validated — FIXED](regression-guards/disabled-entry-conditions-not-compiled.md)
- [RequestRestriction predicates log descriptors — FIXED (cl-08)](regression-guards/request-restriction-sync-predicates-log-descriptors.md)
- [Unresolved Condition References Need Consumer Policy — FIXED](regression-guards/unresolved-condition-references-need-consumer-policy.md)

## Review evidence

- [Non-RAP `ipRefs` status diagnostics are already neutral — FALSE POSITIVE (cl-06)](review-evidence/iprefs-status-diagnostic-already-neutral.md)

## Superseded decisions

- [Unified Condition Layer — DESIGN-TRADEOFF](superseded/unified-condition-layer.md)
