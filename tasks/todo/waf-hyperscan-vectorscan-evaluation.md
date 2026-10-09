# Evaluate Hyperscan and VectorScan for WAF candidate filtering

Status: Conditional investigation; P3

Start only if profiling shows a material regex bottleneck after the completed native multi-pattern optimizations.

- Compare x86 Hyperscan and ARM VectorScan capabilities, supported syntax, database/scratch/stream memory, compilation and reload peaks, packaging, licensing, and native failure impact.
- Keep original ordered native confirmation, captures, and conservative fallback; external engines must not silently change semantics.
- The edgion-waf leaf remains safe Rust with no external production WAF engine dependency. Any FFI implementation needs a separately approved boundary and optional crate design.
- Produce a measured go/no-go decision before proposing implementation; this task authorizes investigation only.

Start from `edgion-waf/src/seclang/` and the existing release matcher benchmarks.
