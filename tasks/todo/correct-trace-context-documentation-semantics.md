# Correct TraceContext Documentation Semantics

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-02 |
| Status | pending |
| Type | documentation |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Align TraceContext schema and bilingual documentation with its actual generation gates and W3C sampling semantics.

## Requirements

- Inventory TraceContext Rustdoc, English/Chinese guides, examples, and generated reference text.
- State that `trustInbound: false` discards inbound context, while root generation still depends on `generateWhenMissing`.
- State that `defaultSampled` sets the W3C sampled recommendation bit and does not force downstream sampling.
- Keep examples and field tables symmetric across English and Chinese.
- Do not change runtime behavior or existing integration tests.

## Acceptance criteria

- No documentation claims unconditional generation or mandatory downstream sampling.
- Schema Rustdoc and both user guides describe the same behavior matrix.
- Agent-doc, English-only, format, diff, and relevant repository guards pass.
