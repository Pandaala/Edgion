# Clarify DSL quoted-newline syntax

## Meta
- Status: pending
- Type: docs
- Priority: P3

## Problem
`docs/en/user-guide/http-route/filters/edgion-plugins/dsl/language-syntax.md` says multiline string literals are unsupported, but `edgion-dsl/src/parser/lit.rs` accepts a raw newline between ordinary double quotes.

`parse_string_literal` handles `\n` as an escape and otherwise appends non-quote characters, including an actual newline. The guide's statement appears immediately after its escaped-newline example; it conflates an ordinary quoted string containing a newline with unsupported literal forms.

## Scope
Update the guide to describe current parsing: an escaped newline and a raw newline inside an ordinary double-quoted literal are accepted; raw-string and triple-quote forms have no dedicated syntax. Keep this task limited to documentation and preserve parser/runtime behavior. A proposal to reject raw newlines would be a separate language change with focused parser tests.

## Verification
Compare wording with `parse_string_literal` and `edgion-dsl/tests/unit/parser/expr_test.rs`, then run the documentation consistency check. The final guide must not imply that ordinary quoted strings reject a raw newline.
