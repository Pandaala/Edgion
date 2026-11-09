# LDAP Decoder Fuzzing

This isolated nested Cargo project is not part of the Edgion workspace or any
production dependency graph. The `ldap_decode` target feeds arbitrary server
bytes through the same path `BindSession` uses for every response: the frame
header (`length_octets`, `frame_content_len`, which enforce the 64 KiB cap)
and then the BindResponse, ExtendedResponse or search-frame decoder. With bit
5 of the first byte set, the rest is instead compiled as RFC 4515 filter text
and encoded. The first input byte selects the mode and the expected message ID. The property is that no
input panics, aborts, or recurses; the decoders are straight-line code, so
deeply nested BER cannot grow the stack.

Install `cargo-fuzz`, then run from the repository root:

```text
cargo +nightly fuzz run --target-dir target/ldap-fuzz --fuzz-dir edgion-ldap/tests/fuzz ldap_decode -- -max_len=2048 -timeout=10
```

Turn every crashing or minimized regression input into a unit test in
`edgion-ldap/tests/unit/proto_test.rs`; the local `corpus/` and `artifacts/`
directories are ignored by Git. Record the command, toolchain, seed, duration,
executions, and result with the change being validated. The fuzz dependency
must never move into `edgion-ldap/Cargo.toml`.

The command keeps build artifacts under the repository-root
`target/ldap-fuzz/`; corpus and crash artifacts stay in this directory and are
ignored by Git.

## Recorded run

| Date | Toolchains | Command options | Result |
|------|------------|-----------------|--------|
| 2026-11-09 | cargo-fuzz 0.13.2; nightly 1.100.0 (e457a7b0d 2026-08-27); ASan | `-max_total_time=60 -seed=11092026 -max_len=2048 -timeout=10 -print_final_stats=1` | 64,746,774 executions in 61 seconds; 290 covered edges, 431 features, 1,062 new units, zero slow units, no crash or timeout |
| 2026-11-09 | cargo-fuzz 0.13.2; nightly 1.100.0 (e457a7b0d 2026-08-27); ASan | `-max_total_time=60 -seed=11092027 -max_len=4200 -timeout=10 -print_final_stats=1` (search frames and filter compile added) | 14,457,688 executions in 61 seconds; 724 covered edges, 2,643 features, 7,245 new units, zero slow units, no crash or timeout |
| 2026-11-09 | cargo-fuzz 0.13.2; nightly 1.100.0 (e457a7b0d 2026-08-27); ASan | `-max_total_time=60 -seed=11092028 -max_len=4200 -timeout=10 -print_final_stats=1` (DN parse and equivalence added) | 2,610,629 executions in 61 seconds; 1,209 covered edges, 4,872 features, 4,688 new units, no crash or timeout |
| 2026-11-09 | cargo-fuzz 0.13.2; nightly 1.100.0 (e457a7b0d 2026-08-27); ASan | `-max_total_time=60 -seed=11092030 -max_len=4200 -timeout=10 -print_final_stats=1` (group-requirement filter added) | 2,820,168 executions in 61 seconds; 1,423 covered edges, 5,558 features, 1,975 new units, no crash or timeout |
