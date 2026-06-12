---
name: dsl-bytecode-deserialize-safety
description: Use when reviewing the DSL bytecode load path (CompiledScript::deserialize_base64); covers required size limits, recursion depth, base64 pre-validation, Constant::Regex ReDoS risk, and missing fuzz infrastructure.
---

# DSL Bytecode Deserialize Safety — OPEN / IN-PROGRESS

**Status**: Open — fuzz infrastructure not yet in place; size-limit checks present but regex recompilation risk unmitigated.
**Source TODO**: `tasks/todo/dsl-vm-rhai-research/issues/issue-19-deserialize-base64-fuzz.md`
**Priority**: P2

## Conclusion

`CompiledScript::deserialize_base64` (`edgion-gateway/src/plugins/http/dsl/lang/bytecode.rs:276-307`) is an external trust boundary: bytecode is delivered via K8s CR / gRPC and may be attacker-controlled. The path has partial length guards but lacks fuzz coverage and does not bound `Constant::Regex` recompilation, leaving ReDoS / pathological-compile risk open.

Fix suggestions of the form "serde will handle malformed input gracefully" are **not accepted** — serde-json has a documented CVE surface and does not bound recursion depth by default.

## Core Rationale

**1. Trust boundary**

Bytecode is compiled by the Controller but delivered through K8s CR / gRPC sync. Any actor with `kubectl apply` rights or access to the control plane gRPC channel can supply a crafted base64 payload. The existing length bounds (`bytecode.rs:279-296`) guard against trivially oversized input, but do not prevent:

- Deeply nested or wide JSON that exhausts the serde-json recursion budget.
- A `Constant::Regex` variant whose `pattern` field triggers a ReDoS during `Regex::new(...)` called inside the serde `Deserialize` impl at deserialization time.
- A base64 string that decodes to syntactically valid JSON with a pathological structure.

**2. `Constant::Regex` recompilation**

The `Constant` enum contains a `Regex` variant. When serde deserializes this variant it calls `Regex::new(pattern)` (or equivalent) on the embedded pattern string. A malicious pattern can trigger catastrophic backtracking (ReDoS) or consume unbounded memory during NFA/DFA construction. The fix requires wrapping the regex construction inside the `Deserialize` impl with `regex::RegexBuilder` size limits (`size_limit`, `dfa_size_limit`).

**3. Fuzz infrastructure gap**

As of the investigation date the repository has no `cargo-fuzz` targets (only boringssl C harnesses). The Rhai reference implementation has `fuzz/fuzz_targets/fuzz_serde.rs` at the same level. Without a fuzz harness, latent panics in the serde-json + regex path remain undetected.

## Required Checks

| Check | Required action | Current status |
|-------|----------------|----------------|
| Base64 length bound | Reject payloads above a documented maximum before `from_slice` | Partial — `bytecode.rs:279-296` has guards; exact limits need verification |
| JSON recursion depth | Pre-validate or use a depth-limited deserializer | Not present |
| `Constant::Regex` size limits | Add `RegexBuilder::size_limit` + `dfa_size_limit` in the deserialize impl | Not present |
| Fuzz target | `cargo-fuzz` target exercising `deserialize_base64` with arbitrary input | Not present — see issue-19 for implementation plan |
| Error path | Ensure all `serde_json::from_slice` call sites use `?` not `.unwrap()` | Verify at bytecode.rs:299 |

## Fix Suggestions Not Accepted

- "serde-json automatically handles malformed input" — serde-json returns `Err` for syntactic errors but does not bound recursion depth or execution time.
- "the length check is sufficient" — length-bounded valid JSON can still be deeply nested or contain ReDoS-triggering regex patterns.

## Re-evaluation Triggers

Re-open or escalate if:

- A fuzzer (once infrastructure is in place) finds a panic or OOM in `deserialize_base64`.
- A new `Constant` variant is added that contains a compiled object (regex, pattern, etc.) and derives `Deserialize` without `Deserialize`-time bounds.
- The delivery path for bytecode expands beyond Controller-compiled CR/gRPC to direct user upload.

## Reference Cases

- `tasks/todo/dsl-vm-rhai-research/issues/issue-19-deserialize-base64-fuzz.md` — full implementation plan for the fuzz target; acceptance criteria and CI integration steps
- `edgion-gateway/src/plugins/http/dsl/lang/bytecode.rs:276-307` — target function and existing length bounds
- Rhai reference: `fuzz/fuzz_targets/fuzz_serde.rs` — same-shape fuzz target in Rhai upstream
