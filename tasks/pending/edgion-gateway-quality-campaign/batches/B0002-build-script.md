# B0002 — Build script

## Scope

- Baseline: `8a32f09f074e3280dd9b500810481c62a72613b6`
- Pipeline state: `terminal`
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/build.rs`
- Allowed cross-crate support files: none unless the analyst proves necessity before promotion.
- Forbidden files: every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: `edgion-gateway/build.rs`, this batch record, campaign ledger/main/changelog, and B0001 commit-backfill records.

## Context and value gate

- Analysis subagent: `b0002_analysis`.
- Coverage summary: build script, manifest/lib feature backstops, binary allocator selection, canonical image builder, and FIPS regression guard.
- Concrete opportunity: invalid multiple-allocator builds currently emit the intended `compile_error!` plus an unrelated type-mismatch diagnostic; the build script already owns early feature-conflict diagnostics.
- Decision: `change` (simple), limited to an early allocator mutual-exclusion check in `build.rs`; zero allocator features remain valid and FIPS preflight remains forbidden here.
- Freshness: revalidated on `8a32f09f074e3280dd9b500810481c62a72613b6`; B0001 preserved allocator feature semantics and resolved its FIPS comment drift.

## Implementation

- Implementation subagent: `b0001_review` in a distinct B0002 implementation turn.
- Complexity: simple.
- Plan: count the three allocator feature environment variables and fail early when more than one is enabled, preserving zero/single allocator behavior and the existing TLS check.
- Actual change: added build-script preflight for multiple allocator features using the existing lib-level diagnostic text; zero/single allocator selection and the TLS preflight remain unchanged.
- Focused verification: conflicting allocators now fail only with the custom diagnostic and no `E0308`; zero allocator, default lib, formatting, English-only, and frozen-test checks passed; invalid dual TLS retains its intentional custom failure.

## Independent review

- Reviewer: `b0001_adjudicate` in a distinct B0002 review turn.
- Verdict: LGTM; no Must Fix or Should Fix.
- Review note: concurrent Pingora errors for invalid dual TLS are pre-existing Cargo scheduling behavior and do not undermine the allocator diagnostic goal.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths | passed | Baseline unchanged before delegation. |
| Focused feature matrix | passed | Zero/single allocators pass; every conflicting allocator combination produces only the intentional diagnostic and no `E0308`. |
| Formatting and workspace compile | passed | `cargo fmt --all`; `cargo check --workspace --all-targets`. |
| Workspace lint | passed | Exit 0 with pre-existing warnings outside this batch. |
| Repository guards | passed | Agent docs, SSA, metrics, Pingora source, Gateway no-tracing, English-only, and full `cargo deny check`. |
| Frozen integration paths after verification | passed | Existing `edgion-tests/**` content and modes unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
