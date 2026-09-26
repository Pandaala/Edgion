# Edgion Quality Campaign

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-19 |
| Status | in-progress |
| Type | refactor |
| Priority | P2 |
| Original baseline | `a0b4d49b298149422c7e46150a0ca027ad5321f7` |
| Common phase baseline | `18e03b4c65b5225d0b299db55b8fb17c65232df5` |
| Active anchor crate | `edgion-common` |

## AI guide summary

The user invoked the campaign for `edgion-common` on 2026-09-26. Continue the existing campaign after the completed Gateway phase. Analyze every Common file with separate value, implementation, and independent review roles. Commit only batches passing all gates; never push. Preserve prior Gateway coverage as historical evidence, not a fresh verdict on subsequent external changes.

## Scope and authority

- Current authorized phase: all tracked Common files and confirmed product-source additions, with exact analyst-approved support paths only.
- Ordered phases: `edgion-gateway` (completed historical phase), `edgion-common` (active). Other crates are not independently authorized by this invocation.
- Existing `edgion-tests/**` files are frozen; reconcile only externally committed, individually proven fingerprint changes.
- Preserve behavior, schemas, protocols, architecture, and all applicable red design rules. No finding-repair workflow, unrelated cleanup, or user-work staging.
- The explicit campaign invocation authorizes automatic local commits after verification.

## Campaign state

- Campaign status: `in-progress`.
- Coverage planner: `common_coverage`; 90-file inventory verified; process complete relative paths lexically, with no exclusions.
- Common inventory: 90 files; 1 `analysis-in-progress`, 2 `analysis-ready`, 2 `changed`, 1 `deferred`, 1 `examined-no-change`, 83 `pending`.
- Historical Gateway ledger: 582 rows, all terminal; 504 imported exclusions, 49 changed, 27 examined-no-change, 1 deferred, 1 removed.
- Active serial batch: B0078 verified; automatic commit pending.
- Analysis queue: B0079 certificate utilities (analysis-ready), B0081 certificate duration (analysis-in-progress), then B0080 expiry watcher (analysis-ready; D0018 recorded). IDs reflect allocation; promotion follows lexical paths.
- Resume gate: `freeze_diagnostic` proved 147 changed and 320 new integration files against external commits after `0b5890414bf0e9a9d2459fb318e509058632f624`; 3,305 unchanged entries were retained. The 3,772-entry guard passes. Per-path provenance is in `integration-reconciliation-2026-09-26.tsv`; no test file was edited.

## Protected pre-existing work

- `tasks/ai-2.0/00-context.md`
- `tasks/ai-2.0/01-product-boundary.md`
- `tasks/ai-2.0/02-current-capabilities.md`
- `tasks/ai-2.0/03-competitive-analysis.md`
- `tasks/ai-2.0/04-gap-analysis.md`
- `tasks/ai-2.0/05-roadmap.md`
- `tasks/ai-2.0/06-task-breakdown.md`
- `tasks/ai-2.0/07-decisions.md`
- `tasks/ai-2.0/08-sources.md`
- `tasks/ai-2.0/09-consolidation.md`
- `tasks/ai-2.0/ai-2.0.md`
- `tasks/todo/edgion_io_2_edgion_dev`
- `tasks/todo/waf-architecture-and-multi-pattern-matching.md`

## Records

- `00-context.md`: workflow and progressive knowledge entry points.
- `03-subtasks.md`: retained Gateway history and complete Common inventory.
- `07-changelog.md`: completed batch audit trail.
- `deferred/INDEX.md`: coordinator-owned follow-up tracking.
- `integration-freeze.tsv`: individually verified existing integration-file fingerprints.

## Common coverage plan

Planner `common_coverage` verified 2 package/build files, 48 source files (including two maintained proto inputs), and 40 test/support files. Process full paths lexically, beginning with Cargo.toml and build.rs. Read associated unit tests with their source, but terminate test rows only after full-file examination.

Required context boundaries: proto wrappers/build inputs; payload wire/size limits; DNS transport/parser/fallback; canonical outbound TLS/client/default trust; etcd generation/timeout/election; health publication and Gateway runner; recorder/cardinality installation; panic hook/logging readiness. These are read boundaries, not blanket edit permission. No generated or fixture exclusions apply.
