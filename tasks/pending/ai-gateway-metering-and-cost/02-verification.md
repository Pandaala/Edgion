# Verification

## Targeted checks

- [x] AiProxy quota schema/default/validation tests
- [x] Fixed-point pricing and CRD coverage tests
- [x] Redis cumulative quota protocol tests
- [x] Threshold, timer, coalescing, and final-correction worker tests
- [x] AiProxy request-path regression tests
- [x] Usage and logging tests
- [x] Real Redis integration coverage

## Final checks

- [x] `cargo fmt --all`
- [x] `cargo check --workspace --all-targets`
- [x] `cargo clippy --workspace --all-targets`
- [x] Agent-document consistency guard
- [x] SSA force guard
- [x] Metrics inventory guard
- [x] Pingora source policy guard
- [x] CRD coverage guard
- [x] Gateway tracing guard
- [x] English-only guard
- [x] `git diff --check`
