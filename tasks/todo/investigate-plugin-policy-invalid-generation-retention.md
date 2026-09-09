# Investigate Plugin Policy and Invalid-Generation Retention

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | investigation |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Independently determine whether whole-generation last-known-good retention can preserve an older runtime without newly imposed plugin policy restrictions. The call paths below identify a concrete concern, not a reproduced defect.

## Requirements

- Trace Controller policy stamps, Gateway compilation and `retain_previous_on_invalid_update`, policy sealing, and both `PluginStore::apply_delta` and `replace_all`.
- Reproduce an existing activatable unblocked generation followed by a new Block stamp plus an unrelated retainable invalid DSL/WAF entry. Observe the actually published runtime.
- Verify intended precedence between operator policy and whole-resource retention from original requirements and review knowledge; do not infer it solely from current code.
- Include policy removal, Bypass, initial publication without a previous generation, and corrected subsequent updates in the decision matrix.
- If a defect is confirmed, propose one immutable publication fix with deterministic unit regressions, preserving access-log/metric ownership and static generic rejection bodies.
- Leave existing integration tests unchanged. Record a separate integration issue only if unit evidence cannot prove the relevant boundary.

## Acceptance criteria

- Independent evidence confirms or rejects the suspected enforcement gap for both update paths.
- A verified requirement establishes policy/retention precedence.
- Any necessary fix is scoped and independently reviewed, or a reasoned no-change conclusion documents the retained contract.
