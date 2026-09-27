# Edgion Quality Campaign

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-19 |
| Status | blocked |
| Type | refactor |
| Priority | P2 |
| Original baseline | `a0b4d49b298149422c7e46150a0ca027ad5321f7` |
| Common phase baseline | `18e03b4c65b5225d0b299db55b8fb17c65232df5` |
| Active anchor crate | `edgion-common` |

## AI guide summary

The user invoked the campaign for `edgion-common` on 2026-09-26. Continue the existing campaign after the completed Gateway phase. Use the invoked quality-improvement workflow to examine every Common file and independently review each batch. Commit only batches passing all gates; never push. Preserve prior Gateway coverage as historical evidence, not a fresh verdict on subsequent external changes.

## Scope and authority

- Current authorized phase: all tracked Common files and confirmed product-source additions, with exact analyst-approved support paths only.
- Ordered phases: `edgion-gateway` (completed historical phase), `edgion-common` (active). Other crates are not independently authorized by this invocation.
- Existing `edgion-tests/**` files are frozen; reconcile only externally committed, individually proven fingerprint changes.
- Preserve behavior, schemas, protocols, architecture, and all applicable red design rules. No finding-repair workflow, unrelated cleanup, or user-work staging.
- The explicit campaign invocation authorizes automatic local commits after verification.

## Campaign state

- Campaign status: `blocked` (review queue exhausted; B0084 live verification unavailable).
- Coverage planner: `common_coverage`; 90-file baseline inventory verified; one moved integration target added; process complete relative paths lexically, with no exclusions.
- Common inventory: 91 historical/current rows (90 current files, one moved-away row): 14 changed, 64 examined-no-change, 3 deferred, 9 blocked, 1 removed; no pending files.
- Historical Gateway ledger: 582 rows, all terminal; 504 imported exclusions, 49 changed, 27 examined-no-change, 1 deferred, 1 removed.
- Active serial batch: B0079/B0081/B0080 and B0082 committed as `ee0038cb7`; B0083 DNS committed as `99b85e31b`; B0085 tested and independently reviewed; preparing final local commit.
- Next source batch: B0084 TLS/Etcd fixes are blocked on real-service verification and remain uncommitted; B0085 foundation fixes passed tests and final resumed independent review. Associated ExtProc files were reviewed with synchronization bindings to share the protocol contract context.
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

## Resume checkpoint — 2026-09-27

Worktree: `/Volumes/ExtStore/ws4/Edgion`, branch `feature-08-30`.
Base before final batch: `99b85e31b`. All 90 current Common files and associated
support paths have been reviewed; no unread queue entries remain.

B0084 leaves five Common production files, four tests and its Etcd knowledge page
uncommitted. Static reviews and 109 LinkSys tests pass, but there is no shared
Gateway/Controller/Etcd topology and the local Docker socket is unavailable.
Do not treat unit construction as handshake, dynamic recovery or reconnect proof.
Next: provide the existing shared test environment, run supported RedisTls/Etcd/
Elasticsearch cases and unsupported identity-policy recovery/reconnect checks,
review resulting evidence, then commit B0084 only after the live gate passes.
See `batches/B0084-common-outbound-etcd.md` for exact scope and constraints.

B0085 tests and final workspace check/Clippy pass; its final commit is pending.
D0016/D0018 remain investigation tasks; D0019-D0021 require explicit design choices.
D0017 is separate documentation work. Task creation does not resolve these items.
Protected user files remain unstaged. No remote push is authorized or performed.

Recovery check: the external volume is accessible again. HEAD remains `99b85e31b`;
all 18 B0084/B0085 content fingerprints match the tested working files. No staged
changes or intervening commits were found. The shared services remain absent and
the Docker socket remains unavailable. B0085 final independent review completed with LGTM.
