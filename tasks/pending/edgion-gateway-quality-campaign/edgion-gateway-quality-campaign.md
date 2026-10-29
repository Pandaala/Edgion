# Repository Code Pruning Campaign

## Meta

| Key | Value |
|---|---|
| Created | 2026-10-28 |
| Updated | 2026-10-29 |
| Status | in-progress |
| Type | refactor |
| Baseline | 75c4d51ecd195193ba6a91d210736c0be4e4676f |
| Active anchor | edgion-gateway |
| Active serial batch | R0001 authorized baseline repair |

## Objective and authority

Execute `tasks/todo/repository-code-pruning.md`: semantically inspect every repository-owned code file across all ten workspace crates and repository automation; remove only proven unused code and simplify redundant tests/comments while preserving supported behavior and valuable regression coverage. No deletion quota. User authorizes necessary integration tests, cargo test --all and local commits after independent review and required checks; never push.

On 2026-10-29 the user explicitly authorized fixing baseline failures first, then continuing pruning. This grants the bounded prerequisite exception across Controller, Resources and Gateway for D0002-D0004/D0006-D0008. No further confirmation is needed for these repairs. The original full-repository objective remains unchanged. The goal API exposes no resume operation; its previously blocked metadata does not negate this explicit instruction to resume work.

The user's integration-test scope overrides the skill default freeze: existing integration tests may be simplified only with exact equivalent scenario/assertion/wiring/synchronization/hot-reload/lifecycle evidence. The later user-authorized R0001 repair includes narrowly reviewed integration fixtures and default MetricsLifetime wiring.

## Protected pre-existing work

- Untracked `impl/`: do not edit, delete, stage or claim coverage.
- Untracked `tasks/todo/repository-code-pruning.md`: read-only user-owned spec; do not stage.
- Initial tracked worktree/index were clean. Current implementation changes remain within the independently approved 40-path R0001 allow-list (39 tracked paths and one new script).

## Current execution state

R0001 implementation and independent final review are complete within 40 approved paths. All 107 subprocess wrappers preserve original bodies and existing Gateway/Controller test identities; one intentional capability regression test was added. Latest full workspace tests passed: 13,152 passed, zero failed, 67 ignored across 78 successful target summaries. Workspace check, Clippy (existing advisory warnings), format and static guards passed. TLS real-socket matrix passed three additional repetitions; capability tests passed 31/31; standalone MetricsLifetime passed with owned-process cleanup. Canonical integration preparation completed successfully.

R0001 now passes every required gate. Final untargeted default integration: 290 groups, 279 passed, zero failed or timed out, 11 existing default skips; all isolated and final residue/metrics/requeue guards passed. The stale DSL fixture discovered during default integration was independently repaired and verified in both targeted and full default execution. Six repaired deferred tasks are closed; D0001 and D0005 remain open. R0001 is ready for the authorized local commit, after which B0001 must be revalidated against the new HEAD.

Pruning candidates: B0001(build dependencies), B0002(accounting finalizer), B0003(usage tests) are analysis-ready/held behind R0001 commit. Revalidate their evidence against new HEAD before promotion.14 Gateway targets have analyst evidence, zero have terminal independent acceptance. Gateway ledger has1243 rows. Initial tracked-path inventory has9172 rows; classification is provisional until evidence-based disposition. Inventory/search/compile are never semantic coverage.

## Execution order

Finish Gateway before edgion-algo, edgion-waf, edgion-wasm, edgion-dsl, edgion-resources, edgion-common, edgion-controller, edgion-cli, edgion-tests, then repository automation/executable documentation tooling. The authorized prerequisite repair is the only current exception.

Within each anchor use stable module/file order and actual unit/support mounts. Maintain separate analyst, implementer and independent-review roles; complex changes require plan approval. Keep one serial mutation/review/verification/commit lane and at most three read-only candidate analyses. Freeze exact files, preserve test identities/assertions and protected artifacts, verify and commit before promoting another batch. Reconcile new/changed paths after every drift. Complete only after all scoped code has semantic evidence and all required gates pass.

## Records

- [Context and baseline](00-context.md)
- [Tracked-path inventory](inventory.tsv)
- [Gateway file coverage](03-subtasks.md)
- [Baseline failures and diagnosis](05-issues.md)
- [Changelog](07-changelog.md)
- [R0001](batches/R0001-baseline-repair.md)
- [Deferred index](deferred/INDEX.md)

R0001 verification is complete and pending local commit. No pruning performance or coverage improvement is claimed.
