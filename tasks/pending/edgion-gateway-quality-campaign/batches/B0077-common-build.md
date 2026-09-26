# B0077 — common-build

## Scope

- Baseline: `18e03b4c65b5225d0b299db55b8fb17c65232df5`.
- Pipeline state: analysis-ready.
- Active anchor crate: `edgion-common`.
- Target: `edgion-common/build.rs`.
- Read supporting callers and tests as needed; no implementation scope is approved yet.
- Forbidden: existing `edgion-tests/**`, unrelated modules, and protected user task paths listed in the main campaign record.

## Context and value gate

Analyst `b0077_analysis`: simple change, build.rs only. Replace bare OUT_DIR unwrap with var_os and a contextual propagated error naming OUT_DIR/Cargo; preserve all proto options and paths. Tonic already owns rerun triggers; both RPC sides and Bytes mappings have real consumers. Verify generated artifacts and missing-OUT_DIR executable error, plus required workspace gates. Original knowledge: common layout, conf-sync overview/bounded-payload guard, Rust rules, ext-proc conformance guard, testing. No deferred findings.
