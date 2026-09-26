# B0076 — common-manifest

## Scope

- Baseline: `18e03b4c65b5225d0b299db55b8fb17c65232df5`.
- Pipeline state: in-progress.
- Active anchor crate: `edgion-common`.
- Target: `edgion-common/Cargo.toml`.
- Exact implementation allow-list: `edgion-common/Cargo.toml`. No support files.
- Forbidden: existing `edgion-tests/**`, unrelated modules, and protected user task paths listed in the main campaign record.

## Context and value gate

Analyst: `b0076_analysis`. Decision: change, simple. Full manifest, source/test dependency uses, generated bindings and consumer manifests examined. `kube`, `base64`, and `rcgen` are used only in cfg(test) unit-test modules but declared as normal direct dependencies. Move unchanged declarations to dev-dependencies. Preserve versions/features, h2, test-util, prost-generated requirements and the workspace DAG. This corrects dependency ownership; it does not claim their transitive removal from binaries.

Knowledge: common architecture/core layout/unification, Rust coding, review routing, supply-chain fork/allocator guards, unit testing and local build guidance. Apply red rules and preserve all tests.

## Verification plan

Metadata and depth-1 normal dependency tree; Common library checks with no default features and test-util; Common unit-test compilation; full required workspace checks. No dependency upgrade or lockfile change is authorized.

## Implementation

Implementer `b0076_implement` moved only the three unchanged declarations and corrected the remaining Kubernetes group comment. Metadata/direct dependency tree passed; lockfile unchanged. Independent reviewer `b0076_review`: LGTM, no Must Fix, no disagreements. Required compile/verification gate still running.


## Verification results

- Metadata/direct normal dependency tree passed; Cargo.lock unchanged.
- Common lib checks with no-default-features and test-util passed; all existing Common unit tests compiled with --no-run.
- Applied cargo fmt; final fmt --check, workspace all-target check and Clippy passed.
- Agent docs, SSA, metrics, Pingora policy, transform contract and unit-test layout passed.
- cargo deny passed (existing duplicate/unmatched-ignore warnings); no upgraded dependencies.
- Frozen integration check passed after exact external-commit reconciliation; no test edits.
- Baseline phase setup and queued analysis records are bookkeeping only; queued candidates still require serial promotion and independent review. D0016 remains investigation pending.

## Exact commit allow-list

- `edgion-common/Cargo.toml`
- `tasks/pending/edgion-gateway-quality-campaign/00-context.md`
- `tasks/pending/edgion-gateway-quality-campaign/03-subtasks.md`
- `tasks/pending/edgion-gateway-quality-campaign/07-changelog.md`
- `tasks/pending/edgion-gateway-quality-campaign/edgion-gateway-quality-campaign.md`
- `tasks/pending/edgion-gateway-quality-campaign/integration-freeze.tsv`
- `tasks/pending/edgion-gateway-quality-campaign/integration-reconciliation-2026-09-26.tsv`
- `tasks/pending/edgion-gateway-quality-campaign/batches/B0076-common-manifest.md`
- `tasks/pending/edgion-gateway-quality-campaign/batches/B0077-common-build.md`
- `tasks/pending/edgion-gateway-quality-campaign/batches/B0078-common-allowed-san.md`
- `tasks/pending/edgion-gateway-quality-campaign/batches/B0079-common-cert-util.md`
- `tasks/pending/edgion-gateway-quality-campaign/deferred/INDEX.md`
- `tasks/pending/edgion-gateway-quality-campaign/deferred/D0016-allowed-san-regex-ignore-case.md`
- `tasks/todo/allowed-san-regex-ignore-case-contract.md`

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
