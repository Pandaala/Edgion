# WAF Fuzzing

This isolated nested Cargo project is not part of the Edgion workspace or any
production dependency graph. It checks ordered equivalence across all native
backends and prefilter modes, ranges, anchors, earliest search, overlapping
standard search, pattern-set scans, streaming chunk boundaries, and bounded
replacement. The transaction-body target drives all four bounded request-body
processors through transaction publication and phase-2 execution. The SecLang
pipeline target mutates bounded rule source and transaction input, then requires
the accelerated and scalar-reference paths to agree across all five phases.

Install `cargo-fuzz`, then run from the repository root:

```text
cargo +nightly fuzz run --fuzz-dir edgion-waf/fuzz aho_corasick_equivalence -- -max_len=640 -timeout=10
cargo +nightly fuzz run --fuzz-dir edgion-waf/fuzz transaction_body -- -max_len=4096 -timeout=10
cargo +nightly fuzz run --fuzz-dir edgion-waf/fuzz seclang_pipeline edgion-waf/fuzz/seeds/seclang_pipeline -- -max_len=8194 -timeout=10
```

Preserve crashing or minimized regression inputs in
`fuzz/corpus/aho_corasick_equivalence/`. Record the command, toolchain, seed,
duration, executions, and result in the Stage 8 acceptance ledger. The fuzz
dependency must never move into `edgion-waf/Cargo.toml`.

## Recorded run

| Date | Toolchains | Command options | Result |
|------|------------|-----------------|--------|
| 2026-08-28 | cargo-fuzz 0.13.2; nightly 1.100.0 (e457a7b0d 2026-08-27); ASan | `-max_total_time=60 -seed=8282026 -max_len=640 -timeout=10` | 313,710 executions in 61 seconds; 1,583 covered edges, 5,429 features, no crash or timeout |
| 2026-08-29 | cargo-fuzz 0.13.2; nightly 1.100.0 (e457a7b0d 2026-08-27); ASan | `-max_total_time=60 -seed=8292026 -max_len=640 -timeout=10` | 348,692 executions in 61 seconds; 1,757 covered edges, 6,144 features, no crash or timeout during the subsequently rejected wide-bitmap experiment |
| 2026-08-29 | cargo-fuzz 0.13.2; nightly 1.100.0 (e457a7b0d 2026-08-27); ASan | `-max_total_time=60 -seed=8302026 -max_len=640 -timeout=10 -print_final_stats=1` | Final accepted tree: 336,031 executions in 61 seconds; 1,687 covered edges, 5,863 features, zero slow units, no crash or timeout |
| 2026-08-30 | cargo-fuzz 0.13.2; nightly 1.100.0 (e457a7b0d 2026-08-27); ASan | `transaction_body -max_total_time=20 -seed=8302026 -max_len=4096 -timeout=10 -print_final_stats=1` | 1,262,497 executions in 21 seconds; 2,275 covered edges, 5,745 features, zero slow units, no crash or timeout |
| 2026-08-30 | cargo-fuzz 0.13.2; nightly 1.100.0 (e457a7b0d 2026-08-27); ASan | `seclang_pipeline seeds/seclang_pipeline -max_total_time=30 -seed=8302026 -max_len=8194 -timeout=10 -print_final_stats=1` | 817,159 executions in 31 seconds; 9,205 covered edges, 28,242 features, zero slow units, no crash or timeout; accelerated and scalar-reference outcomes agreed |
