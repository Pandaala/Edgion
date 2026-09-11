# Evaluate an Optional Hyperscan or VectorScan Backend

## Meta

| Key | Value |
|-----|-------|
| Status | pending |
| Priority | P2 |
| Activation | After Stage 8 closure and only with an observed multi-regex bottleneck |

## Goal

Determine whether an optional Hyperscan-family candidate generator materially
improves end-to-end WAF multi-regex throughput without changing rule order,
spans, captures, transformations, interventions, bounded memory, or the native
fallback. Intel Hyperscan is an x86 candidate; ARM64 evaluation must use
VectorScan and verify the exact NEON/SVE build/runtime support being tested.

## Required evidence

- Current ARM64 and x86-64 support, build, packaging, and cross-compilation.
- Block, vectored, and streaming database semantics.
- Database, scratch, per-stream, and reload-overlap memory measurements.
- `hs_database_size`, `hs_scratch_size`, and `hs_stream_size`, including
  concurrent-scan multiplication.
- Exact comparison with Edgion source order, transformations, captures, chains,
  negation, and early interventions.
- Same-host release benchmarks on the pinned internal and CRS workloads.
- License, supply-chain, container-size, FIPS, CPU-dispatch, and native-crash
  impacts.

## Safety boundary

- `edgion-waf` keeps `#![forbid(unsafe_code)]`.
- Any FFI must live in a separate optional crate.
- Callback order never executes WAF actions directly; the backend may only set
  conservative candidate bits for native source-order confirmation.
- Review callback panic isolation, pointer lifetime, non-reentrant scratch,
  stream cleanup, drop order, allocator alignment, and CPU feature checks.
- No platform database is serialized through `conf_sync`.

## Exit gate

An implementation proposal may start only if the investigation proves a
material end-to-end gain, defines bounded worker/request state, preserves exact
native confirmation, and retains a fully tested native fallback.
