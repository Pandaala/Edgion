---
name: resource-schema-transport-contract
description: Mandatory CRD schema, serde transport, schemars visibility, and secret-redaction invariants for Edgion resources.
---

# Resource schema and transport contract

Resource structs serve two independent paths:

1. Kubernetes admission uses the committed structural CRD. Fields absent from that schema can be
   pruned by the API server before the Controller sees them.
2. Controller-to-Gateway conf_sync serializes the processed Rust struct, including resolved values
   needed by the data plane.

Every field must be classified deliberately.

| Field class | CRD/schemars | serde/conf_sync | Additional rule |
|-------------|---------------|-----------------|-----------------|
| Operator-authored | Must be declared in the committed CRD | Serialize normally | Check name, type, nesting, defaults, and validation |
| Controller-resolved value needed by Gateway | `#[schemars(skip)]` | Keep serialized | Do not use `#[serde(skip)]` |
| Controller-resolved secret needed by Gateway | `#[schemars(skip)]` | Keep serialized | Use `redact_opt` / `redact_required` so Admin config output is redacted |
| Pure in-process cache re-derived by the receiver | `#[schemars(skip)]` | `#[serde(skip)]` | Both omissions are intentional |
| Resolver-only secret container | Do not derive `JsonSchema` where possible | Serialize only if transport requires it | Defense in depth against future schema embedding |

`skip_serializing_if = "Option::is_none"` is not a schema-visibility control. It only omits a
`None` value from serde output; use `#[schemars(skip)]` to keep a resolved field out of a CRD.

## Automated guards

`edgion-resources/tests/crd_coverage.rs` compares fixed property names in `spec` and `status`
for every served version of all nine custom CRDs. Its explicit scope inventory must match
the committed CRD file set. The existing bidirectional diff operates on projections that
merge composition branches, reference chains and reference siblings, and traverse array items.
Union projection compares the combined property names, not variant validity. Dynamic map
keys and value schemas are outside this fixed-property check.

| Resource | Fixed-field scope and open boundaries |
|----------|---------------------------------------|
| EdgionGatewayConfig, EdgionTls, EdgionBackendTrafficPolicy, EdgionBackend | Full structural spec and status property names |
| EdgionAcme | Spec/status, including challenge type and gatewayRef.name; autoEdgionTls.parentRefs items remain preserved and undeclared; gatewayRef's group/kind/namespace/port/sectionName remain preserved optional omissions |
| EdgionPlugins | Spec, stage-entry fields, conditions, body/dye declarations, access-log fields, WAF and partitioned status; stage config interiors stay opaque |
| EdgionStreamPlugins | Both stage-entry envelopes and partitioned status; config interiors stay opaque |
| LinkSys | Spec type/config envelope and complete partitioned status; variant config interiors stay opaque |
| EdgionConfigData | Spec including the data type/config envelope, and status; data.config interiors stay opaque |

HTTP condition `ipRefs`, `secretRef` and `secretGroups` bodies are explicitly excluded at
entry, body and dye condition paths. Their containing fields are still compared. Their
items/objects and ACME autoEdgionTls.parentRefs items must retain preserve-unknown and
declare no fixed children; new declarations require a scope update. The partially declared
ACME gatewayRef retains its preserve-unknown requirement and name-field checks. No blanket
preserve-unknown exemption is inferred from the CRD.

The guard retains narrow historical pins for ACME status and the stream-plugin TLS-route
field. Regression specimens remove and add fixed top-level/nested fields in in-memory CRDs,
and demonstrate that open-body internal differences are ignored while missing bodies fail.
The property walker does not validate field types/patterns/defaults.

`edgion-resources/tests/crd_field_contract_parity.rs` separately checks 16 selected
scalar fields across every served EdgionGatewayConfig and EdgionBackendTrafficPolicy
CRD version: retry budget, LinkSys limits, request-body policy, outlier detection,
and active health-check port. It compares types, serde defaults, inherited defaults,
selected numeric bounds, and existing semantic validation, including explicit
health-check port-zero rejection. Explicit CRD-default specimens are not a
Kubernetes admission simulator; runtime-only constraints and live ingestion-mode
equivalence remain distinct coverage boundaries.

Verification on 2026-10-06 passed 97 targeted tests, workspace check/clippy,
formatting, and project guards. Temporary type/default/maximum CRD mutations
triggered the intended assertions and were restored byte-for-byte. This is scoped
historical verification, not full ingestion-mode or live API-server equivalence.
The later direct-input health-check port correction is covered by the
[active health-check port validation guard](../../04-review/backends/regression-guards/active-health-port-validation.md).

`edgion-resources/tests/plugin_crd_type_coverage.rs` and `stream_plugin_crd_type_coverage.rs`
guard open string plugin discriminators and config preservation. Unsupported names are rejected
through typed sentinel validation and fail-closed runtimes, not CRD whitelists. These guards do
not make the plugin `config` bodies structural. Nested condition enums remain separately guarded.

LinkSys `spec.type` and EdgionConfigData `spec.data.type` are also open strings,
guarded by `cicd/checks/check_open_extension_types.py`. Unknown names decode to
schema-hidden invalid markers and withdraw stale runtime state. Known configurations,
credential substructures, ACME challenge types, and resource kinds remain typed.

Fixed envelopes and status fields of tagged-enum CRDs are covered, but intentionally open
config/reference interiors are not. Review their `#[schemars(skip)]`, serde and redaction
annotations manually; use typed validation and dedicated enum/constraint guards for the
contracts that property-presence checks cannot establish.

## Change checklist

1. Classify every new/renamed field using the table above.
2. Update the Rust type, committed CRD, Controller resolver, Gateway consumer, examples, and status
   behavior as applicable.
3. Audit resolved Secret/credential fields for both `#[schemars(skip)]` and a redaction adapter.
4. Run:

```bash
./cicd/checks/check_crd_coverage.sh
cargo test -p edgion-resources --test plugin_crd_type_coverage
cargo check --workspace --all-targets
python3 cicd/checks/validate_agent_docs.py
```

5. For tagged-enum config bodies, manually compare CRD discriminator/nesting and representative
   YAML against serde because the structural guard cannot do it for you.

## Key sources

- `edgion-resources/src/resources/` — serialized resource types
- `config/crd/edgion-crd/` — committed admission schemas
- `edgion-resources/src/redact.rs` — Admin-output redaction adapters
- `edgion-resources/tests/crd_coverage.rs` — structural property guard
- `edgion-resources/tests/plugin_crd_type_coverage.rs` — plugin discriminator guard
