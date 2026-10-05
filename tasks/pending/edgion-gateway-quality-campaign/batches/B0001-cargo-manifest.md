# B0001 — Cargo manifest

## Scope

- Baseline: `a0b4d49b298149422c7e46150a0ca027ad5321f7`
- Pipeline state: `terminal`
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/Cargo.toml`
- Allowed support files: `Cargo.toml`, `Cargo.lock`, `skills/02-features/01-binary-and-deployment/02-feature-flags.md`, `skills/09-misc/SKILL.md`, `skills/09-misc/cicd/00-local-build.md`, `skills/09-misc/cicd/01-docker-build.md`, `skills/09-misc/cicd/02-github-workflow.md`, and `skills/04-review/supply-chain/mandatory-design/pingora-fork-branch-lock-policy.md`.
- Forbidden files: every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: `edgion-gateway/Cargo.toml`, the eight approved support files above, campaign bootstrap/ledger/batch/changelog/deferred records, `tasks/todo/use-explicit-allocator-dependency-features.md`, and `integration-freeze.tsv`.

## Context and value gate

- Analysis subagent: `b0001_analysis`.
- Coverage summary: manifest, workspace dependency graph/lockfile, feature consumers, CI/image builds, and relevant supply-chain/TLS review knowledge.
- Key responsibility/call chain: Gateway target/features and dependencies flow through workspace defaults, `edgion-tests` explicit features, locked CI, and the canonical FIPS builder.
- Concrete opportunity and decisive evidence: unused bundled-SQLite/runtime and test-only dependencies, a duplicate dev dependency, a dead feature, and stale FIPS/Pingora policy text were verified against every consumer.
- Expected benefit: smaller build and supply-chain surface plus accurate supported-build guidance.
- Regression surface: manifest/lockfile and focused English documentation only; supported runtime features remain unchanged.
- Decision: `change`.
- Adjudication: `b0001_adjudicate` resolved the omitted-current-guidance dispute by adding the two exact CI/CD pages above; both contain present-tense stale `edgion_v3` policy and require token-only `edgion_v4` corrections.

## Implementation

- Implementation subagents: `b0001_impl`; adjudicated documentation correction by `b0002_analysis` in a separate B0001 implementation turn.
- Complexity: simple.
- Actual change: removed unused Gateway dependency/dev-dependency declarations, the dead feature, workspace SQLite ownership and its lockfile-only subtree; corrected current FIPS and Pingora branch guidance.
- Behavior intentionally preserved: default jemalloc+BoringSSL, allocator alternatives, FIPS implying BoringSSL, `test-util`, all targets, and the BoringSSL-only product boundary.
- Tests added or updated: none; manifest metadata/tree and build checks provide targeted verification.

## Independent review

- Reviewer: `b0001_review`.
- Verdict: LGTM after correction.
- Must Fix resolution: two omitted current CI/CD pages received the adjudicated one-token `edgion_v3` to `edgion_v4` corrections and passed re-review.
- Deferred observations: workspace-wide explicit allocator dependency features recorded as D0001.
- Adjudication: `b0001_adjudicate` approved the exact two-file support-scope expansion.
- Resume review: `b0001_review` revalidated all approved changes on HEAD `28de28f50`; verdict LGTM with no Must Fix.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths | passed | Baseline contains 3,442 paths. |
| Formatting | passed | `cargo fmt --all` changed no additional path. |
| Workspace compile | passed | `cargo check --workspace --all-targets`. |
| Workspace lint | passed | Exit 0; pre-existing warnings remain outside this batch. |
| Documentation and policy guards | passed | Agent docs, SSA, metrics, Pingora source, and Gateway no-tracing guards. |
| Supply-chain advisory gate | passed after external remediation | `rustls 0.23.45` resolves RUSTSEC-2026-0285; full `cargo deny check` passed on 2026-09-19. |
| Resume/current-HEAD review | passed | External commit retained all nine approved changes; its two extra review-knowledge paths belong to independent security remediation. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: source/support changes absorbed by external commit `28de28f50d37149f83b4174c2be429217e9ca93f`; initial campaign records committed in `8a32f09f074e3280dd9b500810481c62a72613b6`.
