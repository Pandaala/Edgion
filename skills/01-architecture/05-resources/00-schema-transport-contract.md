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

`edgion-resources/tests/crd_coverage.rs` currently performs bidirectional property-presence checks
for three fully structural specs:

- `EdgionGatewayConfigSpec`
- `EdgionTlsSpec`
- `EdgionBackendTrafficPolicySpec`

It also contains narrow regression pins for EdgionAcme status and the stream-plugin TLS-route
field. The property walker does not validate field types/patterns/defaults.

`edgion-resources/tests/plugin_crd_type_coverage.rs` compares the committed EdgionPlugins stage
`type` discriminator entries with `schema_for!(EdgionPlugin)`. It does not make the tagged-enum
`config` bodies structural.

Tagged-enum CRDs such as plugins, stream plugins, ACME, LinkSys, and EdgionConfigData preserve
unknown config fields and are not covered field-by-field. Review their `#[schemars(skip)]`, serde,
and redaction annotations manually.

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
