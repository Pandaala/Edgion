# Evaluate Native SIMD Packed Search

## Meta

| Key | Value |
|-----|-------|
| Status | pending |
| Priority | P3 |
| Activation | Stage 8 profiles show a remaining scalar phrase bottleneck |

## Goal

Evaluate a separately reviewed Teddy-style packed-search backend for ARM64 NEON
and x86 SIMD while preserving the safe scalar implementation as the canonical
fallback and correctness oracle.

## Work

- Measure the remaining gap after TwoGram and RareBytes on realistic rule
  groups and body distributions.
- Define a target-feature dispatch and unsupported-CPU fallback contract.
- Decide whether unsafe intrinsics require an isolated crate or a narrowly
  reviewed module boundary.
- Prove candidate-only behavior with native exact confirmation for every match
  kind, ASCII folding mode, range, overlap, and stream boundary.
- Account for immutable tables and per-request state exactly.
- Run ARM64 and, when supported, x86-64 same-host release comparisons.

## Exit gate

Implementation is allowed only when representative throughput gain materially
exceeds dispatch/cache cost, no workload has an unexplained regression, and the
safe scalar path remains feature-complete and continuously tested.
