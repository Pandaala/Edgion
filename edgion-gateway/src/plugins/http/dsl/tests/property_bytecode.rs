//! Property-based robustness tests for the bytecode pipeline.
//!
//! Coverage:
//!  1. Random bytes through `deserialize_base64` must never panic — it returns
//!     `Result` and that is the entire contract.
//!  2. `serialize_base64 → deserialize_base64 → serialize_base64` is a fixed point
//!     for compiler-produced scripts (double-serialise is idempotent).
//!  3. Random sources that compile reach `Vm::new` without panicking. This is the
//!     key surface the original cargo-fuzz target missed: `Constant::Regex`
//!     re-compilation lives in `Vm::new`, not in `deserialize_base64`, so ReDoS
//!     and pathological-pattern defence belong here.
//!  4. `CompiledScript::line_of(pc)` is bounds-safe for any pc, including past
//!     the end of code.
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

/// Generates a short DSL source string. Templates use the sentinel `__IDX__`
/// so the assembler can substitute a unique per-statement index — this avoids
/// `let a = 1; let a = 2` style collisions that the compiler would reject and
/// keeps the assume-rejection rate low enough for proptest to converge.
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
        prop_assume!(result.is_ok());
        let b64_first = result.unwrap();
        let script = CompiledScript::deserialize_base64(&b64_first)
            .expect("compile_dsl_source output must deserialise");
        let b64_second = script.serialize_base64().expect("re-serialise must succeed");
        prop_assert_eq!(b64_first, b64_second);
    }
}

// ── Property 3: deserialise → Vm::new never panics ──────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn prop_compile_then_vm_new_never_panics(source in arb_source()) {
        let result = compile_dsl_source(&source, &ValidationLimits::default());
        prop_assume!(result.is_ok());
        let script = CompiledScript::deserialize_base64(&result.unwrap()).expect("deserialise");
        // Vm::new returns Result on bytecode it dislikes; we only assert no panic.
        let _ = Vm::new(script, VmLimits::default());
    }
}

// ── Property 4: source_map line_of() is bounds-safe ────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn prop_line_of_never_panics(source in arb_source(), pc in 0usize..8192) {
        let result = compile_dsl_source(&source, &ValidationLimits::default());
        prop_assume!(result.is_ok());
        let script = CompiledScript::deserialize_base64(&result.unwrap()).expect("deserialise");
        // Must not panic for any pc, including past code.len().
        let _ = script.line_of(pc);
    }
}
