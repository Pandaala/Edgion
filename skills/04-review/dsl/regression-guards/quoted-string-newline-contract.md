---
name: quoted-string-newline-contract
description: Use when reviewing DSL quoted-string newline behavior or its syntax documentation.
knowledge_type: regression-guards
---

# Ordinary quoted strings preserve newlines

## Conclusion and scope

EdgionDSL ordinary double-quoted string literals accept both the `\n` escape
and an actual LF inside the quotes. Both produce an LF in the parsed string
value. Crossing a source line does not disable escape processing. There is no
dedicated raw-string or triple-quote syntax.

This guard records a documentation correction, not a parser change. The
user-facing contract is described in the
[English](../../../../docs/en/user-guide/http-route/filters/edgion-plugins/dsl/language-syntax.md#7-string-literals)
and [Chinese](../../../../docs/zh-CN/user-guide/http-route/filters/edgion-plugins/dsl/language-syntax.md)
string-literal guides.

## Rationale and evidence

In `edgion-dsl/src/parser/expr.rs`, `parse_atom` dispatches an opening double
quote to `parse_string_literal` in `parser/lit.rs`. Its escape branch appends LF
for `\n`; its ordinary-character branch appends an actual LF unchanged.
`Expr::string` stores the result as `ExprKind::StringLit(String)` in `ast.rs`.
`compiler/expr.rs` passes that string to `add_str_const` and emits `LoadConst`.

The former guide wording grouped unsupported specialized literal forms with
ordinary quoted strings containing newlines. The corrected bilingual wording
separates those cases and shows equivalent escaped and actual-newline examples.
Documentation parity follows [the documentation standards](../../../11-doc/SKILL.md).

## Alternatives and limits

Rejecting actual newlines would change language behavior and is outside this
documentation task. Such a proposal needs separate authorization and focused
parser tests. This conclusion does not establish indentation stripping,
line-ending normalization, or new escape sequences.

## Verification and re-evaluation

Current source and `edgion-dsl/tests/unit/parser/expr_test.rs` were inspected.
`test_parse_string_literal` covers a single-line literal only; it is not a
newline regression test. No production code or tests are changed by this fix.
Fresh closure checks passed: `cargo fmt --all`,
`cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets`,
and the Python guards `validate_agent_docs.py`, `check_ssa_force.py`,
`check_metrics_inventory.py`, `check_pingora_source_policy.py`,
`generate_gateway_api_transform_contract.py --check`, and `check_unit_test_layout.py`
under `cicd/checks/`. No fresh runtime-test result is claimed.

Re-evaluate if string parsing, escape handling, or literal delimiters change.
Keep both language guides aligned with the resulting behavior.

## Resolution references

- `clarify-dsl-quoted-newline-syntax`: P3 documentation task; corrected the
  description of ordinary quoted newlines while preserving parser/runtime behavior.
