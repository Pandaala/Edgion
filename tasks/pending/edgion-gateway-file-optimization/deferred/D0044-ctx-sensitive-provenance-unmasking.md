# D0044 — Design Sensitive Ctx Provenance and Unmasking

## Meta
- Status: needs-discussion
- Severity: high
- Discovered in: B0071, CtxSet audit
- Category: security-architecture
- Depends on: B0071
- Canonical task: [`tasks/todo/design-ctx-sensitive-provenance-unmasking.md`](../../../todo/design-ctx-sensitive-provenance-unmasking.md)

## Evidence
CtxSet may copy a value from a sensitive source such as `Authorization` or `Cookie` into an arbitrary ctx key. Access-log external-field unmasking is based on explicitly declared output names, while the fixed claims hard-blocklist covers only canonical claims containers. A renamed sensitive value can therefore lose source provenance before a later explicitly allowed ctx lookup.

## Why deferred
A safe correction crosses KeyGet source classification, ctx storage, AccessLogExtern declarations, and access-log masking. It requires a single provenance model that preserves the fixed claims hard-blocklist and explicit-declaration contract without adding a plugin-local masking mechanism or treating every ctx value as sensitive.

## Integration-test impact
No integration issue is opened at discovery. After the design is approved, the implementation plan must decide whether focused cross-module tests are sufficient or whether a standalone integration scenario is needed; existing integration tests remain unchanged during this campaign.
