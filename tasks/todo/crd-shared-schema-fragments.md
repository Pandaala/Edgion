# 4. Maintain shared CRD schema fragments once

## Meta

| Key | Value |
| --- | --- |
| Created | 2026-09-26 |
| Status | pending |
| Type | refactor |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Reduce cross-file duplication of genuinely shared schema fragments, initially Condition and ControllerStatus, through a small deterministic generation/check mechanism if its maintenance benefit is confirmed.

## Request and evidence

The user requested saving each of five CRD optimization proposals as a separate task for later work. This task records a proposal from static inspection, not implementation approval or a newly reproduced defect. Revalidate the cited sources when starting.

HTTP plugin conditions already reuse YAML anchors within one CRD. Shared status fragments still recur across CRDs. Different resource status envelopes have intentional ownership semantics and must not be flattened into one shape.

## Scope and approach

- Inventory exact shared fragments and intentional differences before choosing a canonical source.
- Reuse existing intra-file YAML anchors. Generate only approved repeated fragments into committed standalone CRD manifests.
- Provide deterministic update and --check modes with a small explicit target inventory.
- Preserve resource-specific status ownership, list-map semantics, validation constraints, descriptions, and surrounding handwritten schema.

## Acceptance and verification

- [ ] Each selected shared fragment has one authoritative source and an explicit consumer list.
- [ ] Two generation runs are identical; --check detects stale generated fragments without editing files.
- [ ] Rendered CRDs retain their parsed schema contract, including list keys and bounds; intentional semantic changes are separated and justified.
- [ ] Contributors can update a shared fragment with one command, and checks detect missed regeneration.

## Boundaries

Do not auto-convert every Rust resource into a CRD or create a general templating framework. Do not flatten native conditions, per-controller statuses, per-parent statuses, or policy ancestors into one universal status layout.

## Starting points

Read repository AGENTS.md, skills/SKILL.md, skills/00-design-rules.md, and the relevant resource architecture and review knowledge before implementation.

- [config/crd/edgion-crd/edgion_plugins_crd.yaml](../../config/crd/edgion-crd/edgion_plugins_crd.yaml)
- [config/crd/edgion-crd/edgion_stream_plugins_crd.yaml](../../config/crd/edgion-crd/edgion_stream_plugins_crd.yaml)
- [config/crd/edgion-crd/link_sys_crd.yaml](../../config/crd/edgion-crd/link_sys_crd.yaml)
- [config/crd/edgion-crd/edgion_backend_crd.yaml](../../config/crd/edgion-crd/edgion_backend_crd.yaml)
- [skills/01-architecture/01-controller/10-status-management.md](../../skills/01-architecture/01-controller/10-status-management.md)

## Priority and dependencies

Lower urgency than tasks 5, 2, and 3. Reassess value against actual common-fragment churn before implementation.

Recording this task does not start implementation. No production changes or tests were performed when creating it.
