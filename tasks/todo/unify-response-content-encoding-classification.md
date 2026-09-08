# Unify Response Content-Encoding Classification

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-09 |
| Status | pending |
| Type | bugfix |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Use one allocation-conscious classifier for every response `Content-Encoding`
field and comma-delimited token in Guardrail admission and final-head gating.

## Requirements

- Add a borrowed multi-value response-header access seam or equivalent canonical helper.
- Classify all field lines and comma-delimited tokens case-insensitively.
- Treat identity as eligible only when every effective coding is identity.
- Use the same classifier in Guardrail response admission and response-head Hold gating.
- Preserve fail-open/fail-close behavior, bounded Hold processing, and main-response streaming.
- Add unit coverage for repeated and comma-combined identity/non-identity values.
- Add later wire coverage for both failure policies without editing frozen tests during the campaign.

## Out of scope

- A response-head-only patch that leaves plugin-stage admission inconsistent.
- Buffering the complete upstream response.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-09 | Defer from B0187 | The invariant crosses the session trait/adapter, Guardrail plugin, and final response-head gate. |
