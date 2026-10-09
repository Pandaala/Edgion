# Evaluate native SIMD phrase search

Status: Conditional investigation; P3

Start only if existing TwoGram/RareBytes phrase prefilters remain a measured bottleneck on representative traffic.

- Evaluate ARM NEON and x86 dispatch with a canonical safe scalar fallback; independently review any unsafe boundary.
- Preserve match kinds, ASCII folding, overlapping matches, ranges, chunk boundaries, work limits, and candidate confirmation.
- Compare release throughput, tail latency, compiled memory, and request scratch on the same host.
- Proceed to implementation only when the measured gain justifies architecture-specific complexity.

Start from `edgion-waf/src/aho_corasick/` and its tests/benchmarks.
