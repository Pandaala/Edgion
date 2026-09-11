# Evaluate Premultiplied DFA State Layout

## Meta

| Key | Value |
|-----|-------|
| Status | pending |
| Priority | P3 |
| Activation | A DFA-specific profile demonstrates row multiplication is material |

## Goal

Determine whether premultiplied DFA state identifiers improve matcher hot-loop
performance enough to justify the smaller representable state range and more
complex accounting.

## Work

- Profile the current DFA row-index multiplication on representative ARM64
  workloads.
- Define checked state-ID bounds and construction failures.
- Preserve public state counts, serialization independence, and exact compiled
  memory accounting.
- Compare construction time, retained bytes, p50/p95/p99, and throughput.
- Differentially test all match kinds, folding, ranges, overlaps, and streams.

## Exit gate

Keep the change only if the profile attributes a material cost to the current
layout and the new representation passes exact bounds and semantic parity
without weakening configured limits.
