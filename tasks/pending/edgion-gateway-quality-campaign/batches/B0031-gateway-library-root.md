# B0031 — Gateway Library Root

## Scope

- Baseline: `962cc4061bb6713ada1d5ddb8f7abfe2caf88001`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Anchor file: `edgion-gateway/src/lib.rs`
- Approved cross-cutting support: `edgion-controller/src/lib.rs`
- Forbidden files: every other source file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the two library roots plus B0030/B0031 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Initial analysis subagent: `coverage_plan`; its examined-no-change conclusion was superseded by independent review evidence.
- Independent finding: with the default jemalloc feature and `target_env="msvc"`, each binary intentionally omits the jemalloc global allocator and uses Rust's system allocator, but Gateway and Controller `allocator_name()` functions excluded both the jemalloc return arm and the no-feature fallback. Each function therefore type-checked as `()` instead of `&str`.
- Adjudication: confirmed actionable. The repository does not exclude Windows/MSVC and contains deliberate Windows paths. Synthetic final-crate MSVC cfg compilation reproduced E0308 in both crates without installing a target.
- Cross-cutting scope: Gateway and Controller use package-local parallel allocator features, binary fallback declarations, startup fields, and identical reporting functions. Fixing only the active anchor would knowingly leave the same invariant broken in Controller; CLI has no reporting API and needs no edit.
- Preserved boundaries: production allocator declarations and logging remain binary-owned, BoringSSL remains the supported Gateway backend, rustls remains dormant, public module consumers and the test allocator remain unchanged.

## Implementation

- Added the identical jemalloc-feature plus MSVC arm to both reporting functions, returning `system (default)` to match the already active allocator.
- Changed no feature contract, allocator declaration, CLI, build script, documentation, logging call, or test file.

## Independent review

- Reviewer: `b0001_review`; the same independent reviewer found the defect and reviewed the correction.
- Verdict: approved after checking all eight native/MSVC by none/jemalloc/mimalloc/system states, cfg exhaustiveness and mutual exclusivity, binary declarations, CLI exclusion, truthful reporting, exact cross-crate scope, and hard rules.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused checks | passed | Synthetic MSVC rustc checks passed for Gateway and Controller; default, system, mimalloc, and no-explicit-allocator variants retained their expected behavior. |
| Independent review checks | passed | Eight-state synthetic allocator matrix, formatting, diff, exact scope, and frozen checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing Kafka/frozen-test lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: `63a48dc1eb0fe06f91a45dc640052317acf628c2`.
