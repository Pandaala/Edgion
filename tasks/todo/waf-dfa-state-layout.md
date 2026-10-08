# Evaluate premultiplied native DFA state identifiers

Status: Conditional investigation; P3

Start only if profiling attributes material cost to native Aho-Corasick DFA row-index arithmetic.

- Evaluate premultiplied state identifiers with checked construction, reduced identifier range, and exact memory accounting.
- Preserve matching semantics, all match kinds, and existing construction/runtime limits.
- Compare representative release workloads and adversarial inputs before accepting a layout change.
- This concerns the native phrase DFA; it does not authorize replacing regex PikeVM with an unbounded regex DFA.

Start from `edgion-waf/src/aho_corasick/dfa.rs`.
