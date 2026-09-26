# B0077 — common-build

## Scope

- Baseline: `18e03b4c65b5225d0b299db55b8fb17c65232df5`.
- Pipeline state: in-progress.
- Active anchor crate: `edgion-common`.
- Target: `edgion-common/build.rs`.
- Exact code allow-list: `edgion-common/build.rs`; no support paths.
- Forbidden: existing `edgion-tests/**`, unrelated modules, and protected user task paths listed in the main campaign record.

## Context and value gate

Analyst `b0077_analysis`: simple change, build.rs only. Replace bare OUT_DIR unwrap with var_os and a contextual propagated error naming OUT_DIR/Cargo; preserve all proto options and paths. Tonic already owns rerun triggers; both RPC sides and Bytes mappings have real consumers. Verify generated artifacts and missing-OUT_DIR executable error, plus required workspace gates. Original knowledge: common layout, conf-sync overview/bounded-payload guard, Rust rules, ext-proc conformance guard, testing. No deferred findings.

Current-HEAD revalidation by original analyst passed at `f0ec920dcaf97f3c21c3f5c5ae5e7be5a6d06b09`; test-dependency moves do not affect codegen.

## Implementation and independent review

- Implementer: `b0077_implement`; only build.rs changed.
- Reviewer: `b0077_review`; LGTM, no Must Fix or disagreement.
- D0017 records a nonblocking pre-existing conf_sync overview discrepancy outside this code scope.
- Missing-OUT_DIR execution returned a contextual error with no panic; normal generation produced both Rust bindings and both descriptors.
- Applied fmt; final fmt check, workspace all-target compile/Clippy, docs, SSA, metrics, source policy, transform contract, unit layout and frozen checks all passed.

## Exact commit allow-list

- `edgion-common/build.rs`
- `tasks/pending/edgion-gateway-quality-campaign/03-subtasks.md`
- `tasks/pending/edgion-gateway-quality-campaign/07-changelog.md`
- `tasks/pending/edgion-gateway-quality-campaign/edgion-gateway-quality-campaign.md`
- `tasks/pending/edgion-gateway-quality-campaign/batches/B0076-common-manifest.md`
- `tasks/pending/edgion-gateway-quality-campaign/batches/B0077-common-build.md`
- `tasks/pending/edgion-gateway-quality-campaign/batches/B0080-common-cert-expiry-watch.md`
- `tasks/pending/edgion-gateway-quality-campaign/deferred/INDEX.md`
- `tasks/pending/edgion-gateway-quality-campaign/deferred/D0017-conf-sync-operational-limits.md`
- `tasks/pending/edgion-gateway-quality-campaign/deferred/D0018-conf-sync-client-expiry-identity.md`
- `tasks/todo/align-conf-sync-operational-limits-overview.md`
- `tasks/todo/conf-sync-client-expiry-watch-installed-identity.md`

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
