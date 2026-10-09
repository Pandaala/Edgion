---
name: reference-grant-validation-controller-scoped
description: Use when reviewing ReferenceGrant validation scoping or EdgionGatewayConfig CRD findings; covers why the toggle is controller-global and must not be re-added to the EdgionGatewayConfig CRD.
status: fixed
finding_id: crd-03
closed: 2026-07-17
knowledge_type: intentional-design
---

# ReferenceGrant validation is a controller-global policy — DESIGN-DECISION

## Conclusion

ReferenceGrant cross-namespace validation is controlled by exactly one field:
`ValidationConfig.enable_reference_grant_validation` in the controller YAML
(`edgion-controller/src/config/mod.rs`), default `true`, read through
`is_reference_grant_validation_enabled()`. It is a cluster-wide operator policy,
not a per-`EdgionGatewayConfig` setting.

A duplicate `EdgionGatewayConfigSpec.enable_reference_grant_validation` field
once existed on the CRD with no runtime reader anywhere in the workspace. It was
deleted in crd-03 (2026-07-17). Fix suggestions of the form "add
`spec.enableReferenceGrantValidation` to EdgionGatewayConfig" or "wire the CRD
field to the validation sites" are **not accepted** without a product decision
that per-config scoping is actually wanted.

## Core Rationale

**1. Every enforcement site is controller-global**

`ref_grant/validator.rs`, `ref_grant/edgion_config_data_grant.rs`,
`handlers/dns_resolver_guard.rs`, `handlers/edgion_tls.rs` and
`handlers/gateway.rs` use `is_reference_grant_validation_enabled()` directly or,
for native Gateway TLS fields, through `credential/pipeline.rs`, which
reads the process-global `CONTROLLER_CONFIG` and falls back to `true` when the
config is uninitialized. None of them has an `EdgionGatewayConfig` in scope, so
a per-config toggle would require threading the resource through five unrelated
validation paths — a substantially larger change than the schema field suggests.

**2. Fail-closed default belongs to the operator, not the workload**

The `true` default and the uninitialized-config fallback to `true` are a
deliberate fail-closed security posture. Moving the toggle onto a namespaced-ish
workload resource would let a config author weaken cross-namespace reference
validation, which inverts who owns the decision.

**3. A CRD field with no reader is worse than no field**

The deleted field defaulted to `false` on the CRD while the live control
defaulted to `true`. An operator reading the CRD to answer "are my
cross-namespace refs validated?" got the opposite of the truth, and setting the
field to `true` did nothing. The `crd_coverage.rs` guards check field *presence*
in both directions and are blind to this class — they cannot tell a wired field
from a dead one.

## Fix Suggestions Not Accepted

- "Add `spec.enableReferenceGrantValidation` back to the EdgionGatewayConfig CRD" — reintroduces a field no code reads; the controller YAML field is the control.
- "Align the CRD default with the Rust default" — the alignment that motivated crd-03; it makes a dead field's documentation self-consistent while still contradicting real behavior.
- "Add a `crd_coverage.rs` test pinning the CRD default of a spec field" — a guard pointed at the wrong target if the field has no runtime reader; check for a reader first.

## Re-evaluation Triggers

Re-open this decision only if:

- Per-`EdgionGatewayConfig` (rather than per-controller) scoping of reference validation becomes an explicit product requirement.
- A validation site gains access to the owning `EdgionGatewayConfig`, making the threading cost comparable to the controller-global read.

## Reference Cases

- crd-03 (center-resource-ui-contract review, 2026-07-16; closed 2026-07-17) — deleted the dead CRD field, its `Default` entry, the pinning test, and the docs naming it.
- `edgion-controller/src/config/mod.rs` — `is_reference_grant_validation_enabled()` and `default_reference_grant_validation()`.
- `docs/en/ops-guide/configuration/controller-config.md` — the documented `enable_reference_grant_validation` control.

## Resolution references

- `crd-03` — 2026-07-17.

## Native Gateway TLS pipeline boundary

Gateway backend/serving Secret and frontend CA fields now enter the canonical
bound credential pipeline through actual-owner `GatewayTlsRefs` loans. Its finite
private field positions preserve the existing disabled controller policy only
for these Gateway TLS fields. Source/target validity and exact dependency-owner
matching precede this decision; valid recovery edges are retained even when
Grant enforcement is disabled. A disabled-policy revision marker is synchronous:
these Gateway field tokens cannot freeze or establish asynchronous authority.

This migration does not make every bound credential consumer honor that bypass.
In particular, existing plugin and CredentialSource authorization/freshness
contracts remain unchanged. A separate-process test initializes the real disabled
controller configuration, verifies Gateway recovery and invalid zero effects,
and verifies that an unrelated plugin contract still requires its Grant.
