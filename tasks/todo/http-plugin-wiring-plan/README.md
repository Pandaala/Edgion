# HTTP plugin wiring: investigation and implementation plan

## Status and scope

- Date: 2026-09-25.
- Inspected baseline: `dd66c7e2`; revalidate against HEAD before implementation.
- Profile: HTTP and Stream open-type implementation completed; see the active implementation record for checks and baseline test failures.
- Request: investigate how to reduce the repeated edits needed to add an EdgionPlugins plugin, propose a plan, and assess its value.
- Active implementation: [plan and verification record](03-implementation-plan.md). Earlier investigation measurements remain historical.
- Follow-up: [Envoy filter design comparison](01-envoy-comparison.md), including proposed refinements and adoption boundaries.
- Current recommendation: prioritize the [open CRD plugin types spec](02-open-crd-plugin-types-spec.md), so future plugin names require no CRD changes. Name reuse and a gated factory-dispatch prototype remain independent maintenance candidates. The former CRD generator is superseded.
- Recommended scope: HTTP plugin declaration and cold-path construction wiring. Stream connection and TLSRoute type admission are now included by explicit user request; native top-level WAF and unrelated runtime wiring remain outside scope.

## Evidence and maintenance baseline

The current `EdgionPlugin` enum has 48 variants. The older central-match decision describes 33 variants; its safety rationale still applies, but its size description is historical. This is an extension-maintenance proposal, not a claim that the central match is a correctness defect.

For a FaultInjection-shaped plugin (ordinary request stage, no resource resolution or special capabilities), the following existing public wiring locations contain its entry:

| File | Selected independently maintained locations |
| --- | --- |
| `edgion-resources/src/resources/edgion_plugins/edgion_plugin.rs` | Enum, body requirement, type name, dye capabilities, dye prefix, key-header sources, validation: 7 |
| `edgion-resources/src/resources/edgion_plugins/plugin_configs/mod.rs` | Configuration module and exports, counted together: 1 |
| `edgion-resources/src/resources/edgion_plugins/mod.rs` | Resource exports: 1 |
| `edgion-gateway/src/plugins/http/mod.rs` | Runtime module and exports, counted together: 1 |
| `edgion-gateway/src/plugins/runtime/plugin_runtime.rs` | Imports, four stage factories, plugin name: 6 |
| `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs` | ConfigData denial classification: 1 |
| `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/san_guard.rs` | Outbound TLS validation classification: 1 |
| `config/crd/edgion-crd/edgion_plugins_crd.yaml` | Request-stage discriminator: 1 |

This is a selected lower-bound inventory of 19 locations in 8 shared files, not a historical measurement of engineering time. It excludes plugin implementation files, fine-grained import/export edits, tests, documentation, and special integration work. Non-request-only plugins also need stage metadata edits in `validate.rs`.

Current duplication is not all mechanical:

- `PluginValidation` already owns configuration validation. Gateway `plugin_validation_error` delegates to it and adds a CSRF resolved-key guard. Do not remove that guard when simplifying dispatch.
- `HTTPHeaderFilter` backs two variants with different validators (`validate` and `validate_response`). Validation cannot be inferred from the payload type alone.
- `OpenidConnect` uses a boxed enum payload. Preserve payload types, serialized tags, schema descriptions, and unknown-type rejection.
- `body_requirement`, dye capabilities, and key-header sources depend on configuration values for some plugins. They are not all static flags.
- `ExtProc` and `Guardrail` request entries construct attached response companions before ordinary factory dispatch. Their ordinary factory arms deliberately return `WrongStage`; declaration eligibility must not be inferred from those arms alone.
- `RateLimitLocal` has source-owned outcome-slot binding. `EntryCache` and `EntryBundle` share construction within one immutable source generation and across policy views.
- Native WAF is `spec.waf`, outside `EdgionPlugin`; it must not become an ordinary stage discriminator.
- The Controller's exhaustive negative arms are deliberate admission safety gates. Replacing them with `_ => {}` would reduce edits by weakening review coverage.

## Revised recommendation

The sole objective is fewer manual shared edits when adding an HTTP plugin. Keep existing typed validation, plugin-local constructors, attached callbacks, conditional/body wrappers, failure accounting, source-generation caching, and publication intact. This plan is an investigation deliverable, not an instruction to start implementation.

This revision replaces the earlier P0-P5 sequence. The earlier Resources-first architecture and the 19-to-12 estimate are no longer the recommended first batch. New step names below prevent confusion with those historical labels in the Envoy comparison.

| Item | Decision | Evidence and expected value |
| --- | --- | --- |
| A. Reuse the existing plugin name accessor | Do | All 48 Gateway name mappings exactly equal Resources `type_name`; removes one duplicate dispatch site without a new abstraction |
| B. Open CRD plugin type strings | Bounded prototype | Remove the three name lists once; reuse explicit invalid-runtime handling for unknown types. See the dedicated spec for evidence and scope |
| C. Declare Gateway factory dispatch once | Prototype, then adopt if simple | One ordinary request plugin currently needs one constructor arm and three explicit wrong-stage arms; one centralized row can generate all four exhaustive matches |
| D. Generate Resources enum/name/validation | Defer | Saves about two sites in the same file, but migrates all 48 variants and their docs/schema; body/dye/key exhaustive edits remain |
| E. Automatic module/export wiring | Defer | Current modules export helper types, constants, aliases, and different visibility shapes; simple module-name discovery does not describe these facts |
| F. Capability/Controller/lifecycle redesign | Exclude | Changes semantic ownership and safety review scope rather than mechanical registration |

A and B stand alone. B does not depend on a registry or a new metadata catalogue. C does not require changing constructors. The focused B spec now takes priority over the broader wiring cleanup.

## A. Reuse the existing name accessor

Replace the duplicate Gateway `get_plugin_name` match with the existing `EdgionPlugin::type_name()` call, updating callers directly where clearer. Do not introduce a derive dependency or another name table.

The investigation compared all 48 mappings and found exact equality. Preserve error text and Gateway-only `plugin_validation_error` checks, including the CSRF resolved-key guard.

Acceptance: identical names and diagnostics; one less location to edit for every new plugin. No new test is needed merely to mirror the deleted match; use relevant existing name/error tests.

## B. Open CRD plugin types once

See [the focused specification](02-open-crd-plugin-types-spec.md). The clarified requirement is stronger than removing manual YAML edits: a future plugin name must require no CRD diff or reinstall. The former list-generator proposal is therefore superseded.

The schema has three closed type lists and one already-open stage. The proposed bounded implementation removes the three lists and adds one non-executable unsupported-type sentinel, preserving typed known configs and exhaustive matches. A shared sentinel scan feeds Controller validation and the Gateway's existing fail-closed machinery. No generic watcher rewrite is proposed.

The isolated serde probe succeeded. Actual exhaustive-match edits, schema exclusion, cross-ingress behavior, policy masking, and failure publication must pass the specification's implementation gates. Do not ship enum removal by itself or interpret unknown-field warnings as rejection.

## C. Consolidate factory dispatch without changing construction

Prototype one centralized declaration in `plugin_runtime.rs` that emits the four existing factory functions. Each variant occurs in one row with explicit stage outcomes or a clearly named fixed profile such as `request_only`. A profile mechanically emits the other stages as `WrongStage`; it must never be an implicit fallback for an unlisted variant.

Requirements:

- Preserve exhaustive generated matches over `EdgionPlugin`. Omitting a plugin row must fail compilation.
- Keep the supported-stage overview visible in the declaration. Do not distribute eligibility into default trait methods.
- Preserve existing constructor expressions, arguments, return shapes, and error mapping. A macro may accept explicit bindings for hygiene; it must not force a universal context or constructor signature.
- Keep ExtProc/Guardrail attachment branches, RateLimitLocal outcome-slot binding, all `add_from_*` bodies, and entry caching untouched.
- Attached request owners must have an explicit classification when comparing placement metadata: their direct factories intentionally return `WrongStage`, while their request entries are supported through the existing special branch.
- Check declaration-derived supported placement against the existing Resources stage authority. Do not add a payload-free plugin-kind enum solely to implement this check; a small generated metadata view and a targeted test may suffice.
- Do not add another manually maintained plugin list for the check itself.

The prototype must include an ordinary request filter, response-only filter, DSL/Wasm multi-stage cases, different constructor return shapes, and attached request owners. Generated code must remain easy to diagnose. If achieving this needs recursive macro machinery, a procedural-macro crate, or runtime abstraction changes, do not migrate all variants; retain A/B only.

Acceptance: an ordinary request plugin needs one factory declaration instead of four separately edited matches, with missing-registration compile failure and unchanged stage/failure behavior. Shared-file count does not fall just because four matches become one row; report this as fewer locations, not fewer files.

## Work intentionally left explicit

- Plugin implementation, tests, and documentation.
- Module declarations and curated exports in this first batch.
- Body, dye, routing-key, TLS, and reference-related safety classifications.
- Plugin-specific Secret/ConfigData/Service dependency resolution.

Explicit negative classifications are review decisions, not permission to install wildcard no-op arms. A later automation proposal must retain the decision while reducing repeated entry. It is outside this batch.

Do not add directory scanning, runtime inventory/linker registration, `ConstructedHooks`, mandatory factory files, a universal factory trait/context, generic stage-builder loops, dependency ordering engines, or new request state containers.

## Implementation and verification sequence

1. Implement A independently and verify existing relevant tests and diagnostics.
2. Execute B through the focused spec gates: bounded compile probe, explicit rejection wiring, open CRD schema, replacement guards, and ingest/runtime verification. Open the schema and add rejection handling in the same delivered change.
3. Prototype C and inspect the resulting extension workflow before committing to migration. Verify missing-row compile failure and exact supported placement. Retain the explicit special construction branches.
4. If C passes its simplicity gate, migrate the four factory dispatches with behavior-neutral diffs. Reuse targeted runtime tests for wrong stage, invalid configuration, disabled entries, CSRF resolved state, body-gate ordering, and attached/policy-generation sharing. Do not refresh expected failures blindly.
5. Perform an isolated temporary extension exercise with both an ordinary request plugin and a response-stage plugin. Record manual shared files/locations, CRD changes, commands, and compiler feedback. Do not ship the synthetic plugins.
6. Update the HTTP plugin development instructions to match the resulting workflow. If C is adopted, update directly affected central-match design guidance to distinguish exhaustive generated dispatch from runtime registration.
7. Run workspace fmt/check/clippy and applicable repository guards before landing. Keep unit-test bodies in the required layout. Run targeted tests and relevant shared-topology integration scenarios when warranted; full `cargo test --all` requires an explicit user request.

No Rust tests were executed during this plan review. Read-only verification compared the 48 name mappings and parsed the current CRD stage lists. Documentation checks apply to this deliverable.

## Revised value assessment

Using the same selected 19-location / 8-shared-file ordinary-plugin baseline:

| Delivered scope | Estimated manual locations | Estimated manual shared files |
| --- | --- | --- |
| A only | 18 | 8 |
| A + B | 17 | 7 |
| A + B + accepted C | 14 | 7 |

These estimates exclude business code, tests, documentation, fine-grained imports, and special capability work. B changes the CRD once; subsequent plugin-name additions should leave it unchanged. They are not time measurements, and the extension exercise must verify them. The former 19-to-12 estimate included D and is withdrawn for the recommended batch.

A provides a small mechanical saving. B removes a recurring CRD deployment dependency, but intentionally changes unknown-type ingest and needs the dedicated failure-path verification. C offers the largest additional mechanical reduction in this batch but must justify its macro complexity. D and E may be reconsidered if real extension experience still shows substantial friction; they are not prerequisites.

This first batch is a modest maintenance improvement. It does not yet satisfy a two-file registration target. If reducing seven manual shared files remains insufficient, report the remaining sites and assess a separate bounded tooling proposal instead of expanding the architecture automatically. No throughput, build-time, binary-size, or overall development-time gain is claimed.

## Source and knowledge references

- `AGENTS.md`; `skills/SKILL.md`; `skills/00-design-rules.md`.
- `skills/01-architecture/02-gateway/12-edgion-plugin-dev.md`.
- `skills/01-architecture/05-resources/13-edgion-plugins.md`.
- `skills/02-features/03-resources/13-edgion-plugins.md`.
- `skills/04-review/knowledge-routing.md`.
- `skills/04-review/plugin-runtime/intentional-design/plugin-runtime-god-match.md`.
- `skills/04-review/plugin-runtime/intentional-design/add-from-builders-duplication.md`.
- `skills/04-review/resources/regression-guards/edgion-plugins-crd-type-enum-drift.md`.
- `edgion-gateway/src/plugins/runtime/entry_bundle.rs`.
- `edgion-resources/src/resource/macros.rs` and `resource/defs.rs`.
- `edgion-resources/src/resources/plugin_policy.rs`: existing HTTP policy names come from schema; do not introduce a third manually maintained type list.
