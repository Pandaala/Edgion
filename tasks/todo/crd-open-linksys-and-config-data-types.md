# 1. Open LinkSys and EdgionConfigData extension types

## Meta

| Key | Value |
| --- | --- |
| Created | 2026-09-26 |
| Status | completed |
| Type | feature |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Evaluate and implement open extension discriminators for LinkSys and EdgionConfigData so adding a supported Rust variant does not require updating or reinstalling a CRD type whitelist. Reuse the HTTP/Stream approach only where the resource lifecycle has equivalent semantics.

## Request and evidence

The user requested saving each of five CRD optimization proposals as a separate task for later work. The initial proposal came from static inspection. The user subsequently authorized planning, implementation and multiple review rounds on 2026-09-26.

At task creation, the CRDs enumerated LinkSys spec.type and EdgionConfigData spec.data.type. Their Rust SystemConfig and ConfigEntry enums were strict. Removing only the CRD enum would not provide the plugin system's readable unsupported-type and invalidation behavior.

## Scope and approach

- Trace typed list/watch decoding, validation, status publication, conf_sync, dependency requeues, and consumer invalidation for each resource independently.
- Define unsupported-type representation and status behavior before editing. Distinguish unknown type rejection from advisory unknown-field reporting.
- Verify replacement of a previously valid resource withdraws obsolete clients/data as required by each consumer's existing failure policy. Preserve deliberate overlay fallback and strict trust/WAF policies.
- Open only the approved extension discriminator boundaries; retain validation of known configuration variants and credential fields.

## Acceptance and verification

- [x] A future extension type name is admitted without adding a CRD enum entry; unsupported software reports invalidity without poisoning typed list/watch processing.
- [x] Full and incremental valid-to-unknown-to-valid transitions exercise status, consumer behavior, and stale-state cleanup for both resource families.
- [x] Known malformed configs remain rejected; internal resolved secrets remain schema-hidden and redacted.
- [x] Update extension guides and add focused admission, decoding, Controller, and consumer regression coverage.

## Boundaries

Do not open ACME challenge types, resource kind discriminators, or unrelated nested enums. Do not introduce a generic watcher rewrite or a new failure-policy mechanism. The implementation followed independent design validation and retained these boundaries.

## Starting points

Read repository AGENTS.md, skills/SKILL.md, skills/00-design-rules.md, and the relevant resource architecture and review knowledge before implementation.

- [config/crd/edgion-crd/link_sys_crd.yaml](../../config/crd/edgion-crd/link_sys_crd.yaml)
- [config/crd/edgion-crd/edgion_config_data_crd.yaml](../../config/crd/edgion-crd/edgion_config_data_crd.yaml)
- [edgion-resources/src/resources/link_sys/mod.rs](../../edgion-resources/src/resources/link_sys/mod.rs)
- [edgion-resources/src/resources/edgion_config_data/mod.rs](../../edgion-resources/src/resources/edgion_config_data/mod.rs)
- [tasks/todo/http-plugin-wiring-plan/03-implementation-plan.md](../../tasks/todo/http-plugin-wiring-plan/03-implementation-plan.md)

## Priority and dependencies

Independent of the other four tasks. Medium priority; take up when connector/data-type extension frequency justifies the lifecycle work.

Implementation was authorized on 2026-09-26. The reviewed plan, context, progress,
and verification records are in [the active task directory](../pending/crd-open-linksys-and-config-data-types/01-design.md).

## Completion

Implemented and locally verified on 2026-09-26. Both CRD extension names are open
strings. Unknown types decode to schema-hidden invalid markers with empty config
envelopes, withdraw obsolete state, and recover on a supported reapply. Known
configuration validation and secret redaction remain intact.

Multiple independent review rounds and correction reviews have no unresolved
blocking findings. Final format, workspace check/clippy, guards, 29 contract tests,
76 filtered lifecycle tests, focused requeue/projection tests and both dynamic
scenarios passed. See [verification](../pending/crd-open-linksys-and-config-data-types/02-implementation.md)
and [review decisions](../pending/crd-open-linksys-and-config-data-types/06-decisions.md).

Live Kubernetes admission and `cargo test --all` were not run. No commit or push
was made; PR acceptance remains a separate repository workflow.
