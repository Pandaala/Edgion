---
name: wasm-terminal-safe-inspection
description: Use when changing Wasm CLI text report rendering or guest-controlled diagnostic output.
knowledge_type: regression-guards
---

# Wasm inspection text is terminal-safe

## Conclusion and scope

The CLI text renderer escapes module-controlled import names, hook names and
diagnostic messages with Rust's escape_debug. Canonical analyzer values and
structured report serialization remain unchanged. This is a presentation rule,
not a change to Wasm validation or host compatibility.

## Rationale and evidence

A valid Wasm import can contain ESC, carriage return, newline or C1 control
characters. Even a rejected module must not clear the terminal or forge report
lines. A known function imported from an incorrect module demonstrated this in
the new import inventory; unknown-import diagnostics had the same underlying risk.

Source: edgion-cli/src/cli/commands/wasm.rs::inspect_text. Regression test:
inspect_text_escapes_module_controlled_names_and_diagnostics under the CLI unit
suite exercises real WAT imports/exports and a diagnostic warning, checks line
counts/control bytes, and preserves JSON roundtrip identity.

## Limits and re-evaluation

This covers wasm inspect text, not every CLI error or output surface. Recheck
new fields and renderers at the presentation boundary; do not sanitize binary
identity or structured values before analysis. Verification results are in the
Wasm compatibility implementation record.
