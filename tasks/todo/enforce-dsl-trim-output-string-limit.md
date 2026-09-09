# Enforce the DSL Trim Output String Limit

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-10 |
| Status | pending |
| Type | bugfix |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

The Gateway DSL Trim builtin bypasses the runtime-produced string budget in both its unchanged and trimmed branches. Resolve this local contract gap without adding unnecessary allocations to the unchanged-string path.

## Evidence

- Owner: `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs`, `BuiltinId::Trim` dispatch near line 979.
- Static reproduction (not executed during analysis): use `make_vm_with_limits` and direct `vm.execute` on `trim(" abcdef ")` with `max_string_len = 5`; the current branches return a six-byte result rather than `RuntimeError::StringTooLong { len: 6, limit: 5 }`. `trim("abcdef")` covers the unchanged branch, which also omits the output check.
- The DSL engine security model describes runtime string limits; `skills/03-coding/dsl/builtin-error-semantics.md` defines typed limit failures. Analysis found no explicit Trim exemption. Literal-input exemption does not establish an exemption for runtime-produced output.

## Requirements

- Verify both Trim branches and the original engine/security and builtin-error contracts against current code.
- Enforce the intended output byte cap through canonical string-length helpers, retaining unchanged-string allocation efficiency.
- Add focused exact-limit, over-limit, unchanged-input and multibyte UTF-8 tests with typed error assertions.
- Preserve whitespace semantics, literal input handling, and existing failure-policy mapping.

## Scope

- In scope: Trim implementation and directly related crate-local unit tests; correct owning knowledge only if needed to state the verified contract.
- Out of scope: Split/Range element-budget redesign, unrelated builtins, production logging, schema changes, and existing integration-test edits.
- This needs separate runtime-contract review; it must not be folded into unrelated test-strengthening work.

## Acceptance criteria

- Neither Trim branch returns a runtime string above the established byte cap.
- Exact-limit results succeed and over-limit results report the correct typed error and lengths.
- Focused tests and repository checks pass; existing integration tests remain unchanged.
