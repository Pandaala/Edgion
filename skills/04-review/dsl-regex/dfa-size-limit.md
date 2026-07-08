---
name: dfa-size-limit
description: Use when reviewing DSL `Constant::Regex` compilation paths for DFA memory DoS; documents the 1MB `size_limit` / `dfa_size_limit` cap applied at compiler/validator/VM sites and why bare `Regex::new` is not accepted.
---

# DSL regex constants: DFA size_limit / dfa_size_limit are capped at 1MB

## Conclusion

Fixed (P3-g5-015, 2026-05-16). All three DSL pipeline sites that compile `Constant::Regex` patterns now use `regex::RegexBuilder` with `size_limit(1 << 20)` and `dfa_size_limit(1 << 20)` instead of the bare `regex::Regex::new`:

- `edgion-dsl/src/compiler/emit.rs::Compiler::add_regex_const` — compile-time syntax validation
- `edgion-dsl/src/validator.rs::Validator::validate` — bytecode validation pass
- `edgion-gateway/src/plugins/http/dsl/lang/vm.rs::Vm::new` — pre-compiled regex cache for execution

## Why

The Rust `regex` crate defaults `size_limit` to 10MB per compiled DFA. The DSL validator caps `script.constants` at 256 (`ValidationLimits::max_constants`), so an unbounded build path means a worst case of `256 × 10MB = 2.5GB` allocated synchronously when a malicious bytecode is loaded. Real maximum is closer to `~32_768 / pattern_len × 10MB ≈ 320MB` once `max_const_pool_bytes` (32KB) is taken into account, but it is still a DoS surface under hot-reload churn.

Triggering requires operator-level CRD write access (the path that ships a `DslConfig`), so the realistic threat model is operator compromise rather than per-request abuse. The cap is defense in depth, not the primary security boundary — that remains K8s RBAC over the DSL plugin CRDs.

## Threshold rationale

1 MB DFA per pattern is roughly 1000× the `MAX_REGEX_PATTERN_LEN` (1024 bytes) cap on the source string. Production patterns of ≤1024 bytes typically compile to DFAs well under 100 KB. Only pathological backtracking-explosion patterns (e.g. nested alternation with kleene-stars) approach the ceiling — exactly the patterns this cap is meant to reject.

If a legitimate complex pattern ever exceeds 1 MB DFA, raise the cap consciously rather than removing it. Do not remove the cap.

## Do not re-report

The following finding shapes are now closed:

- "`Vm::new` builds all `Constant::Regex` entries with default 10 MB `size_limit` / `dfa_size_limit`, 256 × 10 MB = 2.5 GB DoS surface"
- "`Validator::validate` calls bare `regex::Regex::new` at the bytecode-validation pass, attacker can blow memory before `Vm::new` is reached"
- "`Compiler::add_regex_const` validates patterns with default DFA cap, the compile-time path is also exploitable"

Verify the three call sites still carry the `RegexBuilder::new(...).size_limit(1 << 20).dfa_size_limit(1 << 20).build()` pattern before re-reporting.

## Generalised rule

Anywhere the gateway compiles a regex from operator-controlled input (DSL constants, future plugin config that accepts user-written regex), prefer `RegexBuilder` with an explicit `size_limit` / `dfa_size_limit` over `Regex::new`. Default 10 MB is too loose for a per-pattern multi-instance scenario.
