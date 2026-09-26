# Correct direct DSL `for ... in range(...)` documentation

## Meta
- Status: pending
- Type: docs
- Priority: P3

## Problem
`docs/en/user-guide/http-route/filters/edgion-plugins/dsl/language-syntax.md` describes direct `for i in range(0, 10)` as iterating string elements subject to the expression builtin's `min(max_list_len, 1000)` truncation. That describes `let nums = range(0, 10)`, not the direct loop syntax.

## Evidence
The parser's `parse_for_stmt` lowers direct `for ... in range(...)` to `StmtKind::ForRange`. `edgion-dsl/src/compiler/loop_.rs` compiles an integer loop variable with `LoopInit`, `LoopBack`, and `LoopEnd`; the Gateway VM enforces its loop and step limits. The expression builtin `range()` instead produces a capped list of numeric strings. The distinction is also reflected in the DSL API reference.

Starting points: `edgion-dsl/src/parser/stmt.rs`, `edgion-dsl/src/compiler/loop_.rs`, `edgion-gateway/src/plugins/http/dsl/lang/vm.rs`, `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs`, and `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/api-reference.md`.

## Scope
Correct the direct-loop section of `language-syntax.md` and check nearby examples for the same conflation. Explain the direct loop's integer variable and VM loop/step limits separately from the expression builtin's string-list and list cap. This is a documentation task; preserve DSL runtime behavior.

## Verification
Compare the final wording with parser lowering, loop compiler, VM limit handling, and the DSL API reference. Check both a direct `for ... in range(...)` example and a `let nums = range(...)` example so the guide assigns the integer loop variable and string-list cap to the correct forms. Run the documentation consistency check.
