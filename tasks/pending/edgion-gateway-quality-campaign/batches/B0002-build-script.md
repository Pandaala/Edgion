# B0002 — Build script

## Scope

- Baseline: `a0b4d49b298149422c7e46150a0ca027ad5321f7`
- Pipeline state: `analysis-ready`
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/build.rs`
- Allowed cross-crate support files: none unless the analyst proves necessity before promotion.
- Forbidden files: every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the owned file if changed, this batch record, campaign ledger/main/changelog, and any analyst-authorized support path frozen before implementation.

## Context and value gate

- Analysis subagent: `b0002_analysis`.
- Coverage summary: build script, manifest/lib feature backstops, binary allocator selection, canonical image builder, and FIPS regression guard.
- Concrete opportunity: invalid multiple-allocator builds currently emit the intended `compile_error!` plus an unrelated type-mismatch diagnostic; the build script already owns early feature-conflict diagnostics.
- Decision: `change` (simple), limited to an early allocator mutual-exclusion check in `build.rs`; zero allocator features remain valid and FIPS preflight remains forbidden here.
- Freshness: must be revalidated after B0001 because the analysis inspected `edgion-gateway/Cargo.toml` and related build documentation that B0001 changes.

## Implementation

- Implementation subagent: pending.
- Complexity: simple.
- Actual change: pending.

## Independent review

- Reviewer: pending.
- Verdict: pending.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths | passed | Baseline unchanged before delegation. |

## Handoff

- Ledger/changelog updated: no.
- Commit: pending automatic commit.
