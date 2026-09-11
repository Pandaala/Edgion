# Record x86-64 Portability Evidence

## Meta

| Key | Value |
|-----|-------|
| Status | pending |
| Priority | P3 |
| Activation | x86-64 becomes a supported Edgion WAF deployment target |

## Goal

Repeat the native correctness, resource, and performance matrix on supported
x86-64 hardware before making architecture-neutral performance claims.

## Work

- Record CPU, OS, toolchain, build flags, and source revision.
- Run native accelerated/reference differentials and resource-bound tests.
- Run the tracked matcher and whole-WAF release benchmarks with the identical
  schema used on ARM64.
- Compare backend and accelerator selection, compiled bytes, scratch, candidate
  diagnostics, and p50/p95/p99 without applying ARM thresholds blindly.
- Investigate architecture-specific regressions before expanding support.

## Exit gate

x86-64 support is declared only after functional parity, bounded-memory gates,
and same-host performance evidence pass on the target deployment class.
