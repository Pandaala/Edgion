# Reconcile the core API group alias contract

## Meta

| Key | Value |
| --- | --- |
| Status | pending |
| Type | investigation |
| Priority | P3 |
| Source | acme-secret-reference-target-validation |

## Evidence and scope

Gateway API SecretObjectReference defaults an omitted or empty group to the
Kubernetes core group. It does not specify `"core"` as an equivalent spelling.
However, `edgion-resources/src/resources/common/mod.rs::is_core_api_group` accepts
both `""` and `"core"`, and its comment attributes that equivalence to Gateway API.
Consumers such as OTLP accept that alias, while the EdgionTls and corrected ACME
Secret target checks accept only omitted/empty group.

Determine whether this is an intentional Edgion extension or contract drift.
Inventory callers and applicable review decisions before changing shared behavior.
The ACME target fix does not authorize a repository-wide alias migration.

## Acceptance

- [ ] Resolve the standard versus Edgion-extension contract with current authority.
- [ ] Correct the shared helper's unsupported standard attribution.
- [ ] If changing semantics, review the exact consumer scope and add regression
      coverage for group identity and ReferenceGrant matching.

Reference: [ACME target identity guard](../../skills/04-review/acme/regression-guards/secret-reference-target-identity.md).
