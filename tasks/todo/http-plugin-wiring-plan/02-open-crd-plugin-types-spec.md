# Open CRD plugin types: bounded design specification

## Status and objective

- Date: 2026-09-25; inspected HEAD: `58848100`.
- Status: implementation authorized; see [active plan](03-implementation-plan.md). The user subsequently included Stream connection and TLSRoute stages; that amendment supersedes the original HTTP-only scope below.
- User requirement: adding an HTTP plugin supported by the binaries must not require editing, generating, or reinstalling the CRD for its new type name.
- Scope: `EdgionPlugins` stage-entry discriminators only. No plugin registry, module discovery, generic watcher rewrite, or Stream plugin migration.
- This specification supersedes step B (CRD list generation) in the parent plan. Generation saves manual editing but still requires a changed CRD, so it does not meet the clarified requirement.

## Findings, in increasing scope

### 1. The schema change itself is small

`config/crd/edgion-crd/edgion_plugins_crd.yaml` has three closed stage-entry `type.enum` lists: request, upstream response filter, and upstream response body filter. The async upstream response stage already uses an open string. Every plugin `config` bag already preserves unknown fields.

Remove those three lists once, keeping `type: string`, required fields, config-object shape, conditions, body, dye, and top-level WAF unchanged. Do not remove nested enums such as condition `KeyGet.type`. Future plugin names need no CRD change. A new common entry field or top-level schema field can still require a CRD update; this promise concerns plugin types and their existing config bags.

### 2. Deleting the lists alone has a real ingest consequence

The Controller uses typed `Api<K>` watchers. Its watcher configuration starts with `watcher::Config::default()`. The locally installed kube-runtime 2.0.1 uses `ListWatch` by default, and its initial list deserializes a typed page before yielding objects. An unknown plugin discriminator currently fails that deserialization; the Controller logs a watch-source error and retains its committed snapshot. A failing initial list can prevent that kind from completing initialization.

This is not an ordinary plugin validation error: `EdgionPluginsSpecShadow::deserialize` must succeed before `validate_plugins` or status handling runs. The existing unknown-field diagnostic is advisory and cannot repair a failed typed decode. The open async stage already exposes this class of input; removing the other enums broadens exposure.

The earlier claim that unknown types simply produce `Accepted=False` was incorrect for this typed Kubernetes input path. Do not use it to justify a YAML-only change.

### 3. A bounded alternative exists without changing the watcher

Prefer one non-executable `Unsupported` unit variant in the existing `EdgionPlugin` enum, with a variant-local deserializer that discards unknown config content. Keep all known variants and their typed configs intact.

Illustrative code, not a production patch:

```rust
fn discard_unsupported_config<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<(), D::Error> {
    serde::de::IgnoredAny::deserialize(deserializer).map(|_| ())
}

// Last variant of the existing adjacently tagged enum:
#[serde(other, deserialize_with = "discard_unsupported_config")]
#[schemars(skip)]
Unsupported,
```

The helper matters: a unit variant alone is not a sufficient assumption for an adjacent `config` object. An isolated offline Rust probe using the repository's serde 1.0.229 tested the helper with the actual `#[serde(flatten)]` entry shape:

| Input | Observed result |
| --- | --- |
| Unknown string type with object config | Decodes as sentinel |
| Config before unknown type, with nested content | Decodes as sentinel |
| Unknown string type without config | Decodes as sentinel |
| Known type with valid config | Retains typed config |
| Known type with invalid config | Rejected, no fallback |
| Missing or non-string type | Rejected |
| Sentinel serialization | Emits `{"type":"Unsupported"}` without original config |

This verifies a serialization mechanism, not repository integration, Kubernetes behavior, or end-to-end rejection. Known malformed configs can still fail typed ingestion; this proposal does not solve every deserialization failure.

The same isolated probe also passed with schemars 1.2.1: `schema_for!(Plugin)` excludes the sentinel when `#[schemars(skip)]` is present. Repository policy-name discovery still needs its own regression test.

## Proposed behavior contract

1. New known plugin: the binaries recognize its enum variant, decode its normal config, and validate it normally. No plugin-name list remains in the CRD.
2. Unknown string type: decode a sentinel so the resource can reach normal processing. Report an indexed validation error and `Accepted=False`. Never construct it as an executable plugin or silently skip it as success.
3. Gateway: mark the resource's compiled plugin runtime invalid using the existing `ReplaceFailClosed` machinery. Publish its existing sealed failure runtime for affected attachments; do not just append a diagnostic string or omit the plugin. This is not a new failure-policy knob.
4. Inspect sentinel entries in all four stages regardless of `enable`, entry conditions, or policy masks. The former strict decoder rejected unknown types even in disabled entries. Preserve that rejection intent and prevent policy bypass from making an unknown entry activatable.
5. Known plugins retain current enable, policy, stage, constructor, validation, and caching behavior. New real variants still require explicit exhaustive match arms; add explicit `Unsupported` arms, never wildcard coverage for future variants.
6. Use `#[schemars(skip)]` to exclude the sentinel from supported-plugin discovery and policy allow/deny names. It is an internal failure representation, not an installable plugin.
7. Unknown config data is discarded, never echoed in status or logs. The original unknown name is also lost in this minimal design. Diagnostics identify the resource, stage, and entry index with a generic unsupported-type message. Kubernetes retains the operator's original spec; typed admin views may show the sentinel. Exact-name preservation is not part of this proposal.
8. A literal `type: Unsupported` is invalid too. Serialization through conf_sync must retain invalidity, never turn the sentinel into a valid empty chain.

## Reuse the existing failure path

The Controller currently validates, updates status, and publishes EdgionPlugins even when plugin validation reports errors. The Gateway is responsible for sealing invalid runtimes. Therefore, `Accepted=False` alone is insufficient, and suppressing Controller publication is unnecessary for this design.

Add one small Resources helper that scans the four entry lists for sentinel entries without filtering disabled entries. Reuse it from `validate_plugins_with_waf` and Gateway `compile_plugins_with_cache`. Avoid duplicate errors from the ordinary enabled-entry validation pass.

At the Gateway compilation entry, record non-retainable invalidity before sealing, in addition to indexed errors. `preparse_errors` alone is explicitly not the activation predicate. `PluginGeneration` masks entries by changing `enable` but keeps their positions and plugin values, so a scan of the effective spec that ignores `enable` can catch the sentinel without changing policy masking or the generic publication infrastructure. Preserve any stronger existing policy-block response precedence.

This is the key reduction in scope: normal typed ingest, workqueues, status persistence, conf_sync, and failure publication remain the transport path.

## Expected production edit surface

| File | Proposed change |
| --- | --- |
| `config/crd/edgion-crd/edgion_plugins_crd.yaml` | Remove three discriminator enums once |
| `edgion-resources/src/resources/edgion_plugins/edgion_plugin.rs` | Sentinel, discard helper, explicit capability/name/validation arms |
| `edgion-resources/src/resources/edgion_plugins/validate.rs` | Shared indexed unknown-type scan, unconditional source validation |
| `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs` | Explicit no-resolution arm where exhaustive ConfigData classification requires it |
| `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/san_guard.rs` | Explicit sentinel classification; no TLS material to resolve |
| `edgion-gateway/src/plugins/runtime/plugin_runtime.rs` | Explicit sentinel factory/name arms; reuse invalid-runtime recording |
| `edgion-gateway/src/plugins/http/plugin_store.rs` | Unconditional sentinel check and fail-closed recording before sealing |

This is an estimated seven production/configuration files, not a compiler-confirmed allow-list. Thirty-two production files reference `EdgionPlugin`; many only use `if let` or selective matches and should not change. Compiler feedback must establish the final exhaustive-match list. Do not modify all consumers preemptively.

Tests and documentation are additional files. In particular, replace the obsolete closed-enum coverage assertions in `edgion-resources/tests/plugin_crd_type_coverage.rs` with an open-discriminator contract while retaining schema-driven stage-exception validation. Keep condition-schema coverage and plugin-policy canonical-name tests.

## Incremental implementation gates

1. **Serialization probe (completed in isolation):** prove the minimal fallback without duplicating the known enum or writing a full custom deserializer. Also verify schema exclusion with the pinned schemars version. No repository production code is changed by this gate.
2. **Compile-scope probe (pending):** in an isolated checkout, add the sentinel and explicit exhaustive arms. Record actual files and errors. Stop and revise this spec if it requires a generic watcher rewrite, a mirrored enum, or broad runtime redesign.
3. **Failure-path wiring (pending):** integrate the shared scan, Controller status, and Gateway invalid-runtime marker. Cover masked and disabled unknown entries. Do not open the CRD in a shippable change until this gate passes.
4. **Open schema and guards (pending):** remove only the three lists; replace list-parity tests with open-string assertions for all four stages and retain the useful stage-authority checks. Validate the real Controller schema-validation path too.
5. **Integration verification (pending):** run the cases below, update affected knowledge and plugin-development instructions, and measure the final edit surface. Land schema and handling together.

## Required verification before implementation is accepted

- Resource JSON/YAML decode, flattened entries, key order, known invalid config, absent/non-string discriminator, literal sentinel, and all four stage lists.
- Schemars and canonical policy-name discovery exclude the sentinel while retaining all real plugin names.
- Controller status reports an indexed invalid-type error without logging raw config; normal resources continue processing.
- Typed list containing one unknown-type resource and one valid resource decodes; Kubernetes initialization completes. Apply, update, delete, and correction of the invalid resource work through existing queues.
- Gateway rejects unknown types in every stage, disabled entries, false conditions, and policy-masked views. Check `is_activatable`, `retain_previous_on_invalid_update`, and actual sealed runtime behavior, not only diagnostic text.
- Valid-to-unknown update replaces execution with the existing failure runtime; correcting it restores normal execution. Unrelated resources remain unaffected.
- conf_sync list/watch and admin/FS/etcd ingress preserve invalidity after sentinel serialization. A schema preflight may still reject unsupported names before typed ingest; document this path-specific feedback rather than forcing every API into identical status semantics.
- Existing wrong-stage and invalid-known-plugin tests remain unchanged unless the CRD admission step intentionally moved to Controller validation.
- One synthetic future plugin in an isolated checkout proves the new extension workflow leaves the CRD byte-identical. Remove the synthetic plugin before delivery.

## Alternatives and decision

| Alternative | Assessment |
| --- | --- |
| Delete CRD enums only | Smallest diff, but leaves unknown names able to break typed listing; not recommended |
| Generate lists from Rust | Preserves admission rejection, but still needs a CRD update for each new name; does not meet the requirement |
| Unit sentinel plus existing rejection paths | Recommended bounded prototype; small serde probe succeeded, repository wiring still needs verification |
| Preserve original unknown name and raw config | Better fidelity, but needs more deserialization/transport design; defer unless indexed diagnostics are insufficient |
| Generic untyped watcher and per-object conversion | Larger infrastructure change; unnecessary for the scoped plugin-name requirement |

One initial CRD update is unavoidable to remove today's lists. Thereafter, deploying binaries with a new plugin needs no CRD update for that name. This does not provide dynamic binary loading or mixed-version compatibility: older binaries without this change can still reject unknown values during conf_sync; upgrade the relevant binaries before using new plugin names.

## Knowledge and policy alignment

The no-catch-all rule in `skills/00-design-rules.md` section Y is a SHOULD rule. This proposal deliberately changes the representation while preserving explicit rejection: no unknown plugin can execute or disappear into a successful chain. The exception is justified by the user's requirement and must be documented if implemented.

`skills/04-review/resources/intentional-design/unknown-plugin-variant-strict-enum.md` correctly warns against silently skipping unknown auth plugins, but its statement that the current unknown-type decoder produces a Kubernetes status condition is not true on the inspected typed watcher path. An explicit sentinel arm also does not inherently remove exhaustiveness for newly added real variants. Correct these statements when updating that decision; do not treat this investigation as already changing the production contract.

Implementation documentation scope: that decision, the discriminator-drift guard page, the schema invariants in `AGENTS.md`, section Y of the design rules, and the HTTP plugin development instructions. Preserve the strict handling of known credential structs and every unrelated schema boundary.

## Validation record

Initial investigation verified the isolated serde/schemars probe and documentation checks. Implementation and fresh repository test results are tracked in [the active plan](03-implementation-plan.md); the original investigation did not include a live Kubernetes test.
