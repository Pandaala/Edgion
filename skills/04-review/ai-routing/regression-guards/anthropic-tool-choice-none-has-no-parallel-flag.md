---
knowledge_type: regression-guards
---

# Anthropic tool choice `none` has no parallel flag

OpenAI permits `tool_choice: "none"` together with
`parallel_tool_calls: false`. Anthropic represents the choice as
`{"type":"none"}`, whose variant accepts no `disable_parallel_tool_use`
member. Parallel tool use is already impossible, so the false flag is a no-op
for this variant.

For `auto`, `any`, and named `tool` choices, a false OpenAI parallel flag still
maps to `disable_parallel_tool_use: true`. When no explicit choice is present,
the same flag synthesizes `auto` before adding the member.

Keep translation tests exact-object based for every variant. Checking only the
`type` field misses provider-invalid extra members.

## Resolution references

- `gw-17` — 2026-08-09.
