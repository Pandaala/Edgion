# Repository Code Pruning Campaign

## Meta

| Key | Value |
|---|---|
| Created | 2026-10-28 |
| Updated | 2026-10-29 |
| Status | in-progress; local code pruning resumed |
| Type | refactor |
| Baseline | 75c4d51ecd195193ba6a91d210736c0be4e4676f |
| Active anchor | edgion-gateway |
| Active serial batch | B0048 implementation planning |

## Objective and authority

Execute `tasks/todo/repository-code-pruning.md`: semantically inspect every repository-owned code file across all ten workspace crates and repository automation; remove only proven unused code and simplify redundant tests/comments while preserving supported behavior and valuable regression coverage. No deletion quota. User authorizes necessary integration tests, cargo test --all and local commits after independent review and required checks; never push.

On 2026-10-29 the user explicitly authorized fixing baseline failures first, then continuing pruning. This grants the bounded prerequisite exception across Controller, Resources and Gateway for D0002-D0004/D0006-D0008. No further confirmation is needed for these repairs. The original full-repository objective remains unchanged. The goal API exposes no resume operation; its previously blocked metadata does not negate this explicit instruction to resume work.

The user's integration-test scope overrides the skill default freeze: existing integration tests may be simplified only with exact equivalent scenario/assertion/wiring/synchronization/hot-reload/lifecycle evidence. The later user-authorized R0001 repair includes narrowly reviewed integration fixtures and default MetricsLifetime wiring.

## Protected pre-existing work

- Untracked `impl/`: do not edit, delete, stage or claim coverage.
- Untracked `tasks/todo/repository-code-pruning.md`: read-only user-owned spec; do not stage.
- Initial tracked worktree/index were clean. R0001 and R0002 are committed locally. The current retention batch owns coordinator records only; prior implementation batches are committed.

## Current execution state

Baseline repairs are committed: R0001 `6407efe23012ad6f53622f7ad53c33c662c1accb`, R0002 `3a5cdf1fc`. R0002 includes local dependency mitigation and the narrowly reviewed ResponseCache synchronization repair. Its final prepared default integration passed 279 groups, zero failures/timeouts, 11 existing skips; affected codec/Wasm and all required local checks passed. The earlier full workspace sweep passed 13,152/0/67 before later targeted phases; it is not represented as a final-state full sweep.

The user explicitly deferred the known 14 upstream Zstd/YAML records and instructed local pruning to continue. Those scans remain failing and D0009 stays open, but they no longer block verified local commits. No ignores, source-policy exceptions or new finding waivers. D0001 and D0005 also remain separately open.

Latest code commits: B0007 d989998e9 removed the unused context setter; B0012 95ca47fdf simplified request validation; B0013 0113f0db4 reused response cleanup; B0016 9ed4637b2 removed redundant evaluator checks (97 before/after tests plus 30 final transport tests). All passed independent review and required checks. Earlier commits and retained files are recorded in the changelog.

B0047 committed as 11ea75781. B0048 is planning removal of the redundant quota test constructor and corresponding guard exception within five frozen paths. B0049 quota runtime retention, B0050 adaptive routing and B0051 candidate analysis are queued.

The Gateway ledger has 1243 rows, with 58 independently accepted terminal targets: twenty changed and thirty-eight examined-no-change. Initial repository tracked-path inventory has 9172 rows. Candidate analysis, inventory and successful compilation are not semantic coverage. No user impl/ or original specification is included in commits; no push. The original repository goal remains incomplete.

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

R0001 is complete and committed. No pruning performance or coverage improvement is claimed.
