//! Property-based robustness tests for the bytecode pipeline.
//!
//! Coverage:
//!  1. Random bytes through `deserialize_base64` must never panic — it returns
//!     `Result` and that is the entire contract.
//!  2. `serialize_base64 → deserialize_base64 → serialize_base64` is a fixed point
//!     for compiler-produced scripts (double-serialise is idempotent).
//!  3. Valid generated sources compile and initialize `Vm` successfully, including
//!     regex-cache construction for one fixed pattern. This does not exercise
//!     pathological patterns or prove ReDoS defenses.
//!  4. `CompiledScript::line_of(pc)` agrees with the expanded source map at every
//!     instruction and returns `None` past the end of code.
//!
//! Out of scope: full `Vm::execute` coverage. Every random script touching the
//! session would require bespoke `MockPluginSession::expect_*` setup; without it
//! the mock panics on unexpected calls and that noise drowns the real signal.
//! Validator-bypass execute panics are already covered by `vm_defensive_tests.rs`.
//!
//! Case counts are sized to keep this file well under 30 s in `cargo test --all`.

use base64::Engine;
use proptest::prelude::*;

use super::*;

// ── Source strategy ─────────────────────────────────────────────────────────

/// Generates a short, valid DSL source string. Templates use the sentinel `__IDX__`
/// so the assembler can substitute a unique per-statement index — this avoids
/// `let a = 1; let a = 2` style collisions that the compiler would reject.
fn arb_source() -> impl Strategy<Value = String> {
    prop::collection::vec(arb_stmt_template(), 0..4).prop_map(|tmpls| {
        tmpls
            .into_iter()
            .enumerate()
            .map(|(i, t)| t.replace("__IDX__", &i.to_string()))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn arb_stmt_template() -> BoxedStrategy<String> {
    prop_oneof![
        any::<i32>().prop_map(|v| format!("let v__IDX__ = {}", v)),
        "[a-zA-Z0-9 ._-]{0,12}".prop_map(|s| format!("let v__IDX__ = \"{}\"", s)),
        Just("let v__IDX__ = req.header(\"X-Test\")".to_string()),
        // Exercises the regex constant pool + Vm::new RegexBuilder path.
        Just("let v__IDX__ = regex_find(\"abc123\", \"\\\\d+\")".to_string()),
    ]
    .boxed()
}

// ── Property 1: random bytes never panic ────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn prop_random_bytes_deserialize_never_panics(
        data in proptest::collection::vec(any::<u8>(), 0..2048)
    ) {
        let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
        // Result is the contract; only requirement is no panic.
        let _ = CompiledScript::deserialize_base64(&encoded);
    }
}

// ── Property 2: double-serialise is idempotent for valid scripts ────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn prop_serialize_deserialize_is_idempotent(source in arb_source()) {
        let result = compile_dsl_source(&source, &ValidationLimits::default());
        prop_assert!(result.is_ok(), "valid source {:?} failed to compile: {:?}", source, result.as_ref().err());
        let b64_first = result.unwrap();
        let script = CompiledScript::deserialize_base64(&b64_first)
            .expect("compile_dsl_source output must deserialise");
        let b64_second = script.serialize_base64().expect("re-serialise must succeed");
        prop_assert_eq!(b64_first, b64_second);
    }
}

// ── Property 3: valid source → deserialise → Vm::new succeeds ───────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn prop_compile_then_vm_new_succeeds(source in arb_source()) {
        let result = compile_dsl_source(&source, &ValidationLimits::default());
        prop_assert!(result.is_ok(), "valid source {:?} failed to compile: {:?}", source, result.as_ref().err());
        let script = CompiledScript::deserialize_base64(&result.unwrap()).expect("deserialise");
        let vm = Vm::new(script, VmLimits::default());
        prop_assert!(vm.is_ok(), "valid source {:?} failed to initialize VM: {:?}", source, vm.as_ref().err());
    }
}

// ── Property 4: line_of() agrees with an expanded source-map oracle ────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn prop_line_of_matches_expanded_source_map(source in arb_source(), pc in 0usize..8192) {
        let result = compile_dsl_source(&source, &ValidationLimits::default());
        prop_assert!(result.is_ok(), "valid source {:?} failed to compile: {:?}", source, result.as_ref().err());
        let script = CompiledScript::deserialize_base64(&result.unwrap()).expect("deserialise");
        let expanded: Vec<u32> = script.source_map.iter().flatten()
            .flat_map(|&(count, line)| std::iter::repeat_n(line, usize::from(count)))
            .collect();
        prop_assert_eq!(expanded.len(), script.code.len(), "incomplete source map for {:?}", source);
        for (valid_pc, &line) in expanded.iter().enumerate() {
            prop_assert_eq!(script.line_of(valid_pc), Some(line), "source {:?}, PC {}", source, valid_pc);
        }
        prop_assert_eq!(script.line_of(pc), expanded.get(pc).copied(), "source {:?}, PC {}", source, pc);
        prop_assert_eq!(script.line_of(script.code.len()), None);
        prop_assert_eq!(script.line_of(usize::MAX), None);
    }
}
