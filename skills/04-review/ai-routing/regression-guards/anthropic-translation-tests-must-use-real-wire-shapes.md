---
knowledge_type: regression-guards
---

# Anthropic translation tests must use real wire shapes

Translation tests must separate values that happen to coincide in trivial
fixtures. In particular, a text block before tool blocks makes Anthropic block
indexes differ from consecutive OpenAI tool indexes. Assert both tool starts
and argument fragments after decoding the emitted SSE, not with JSON substring
ordering assumptions.

Keep exact fixtures for assistant text plus tool calls, tool-only responses
with an explicit null `content`, bare-string and maximum-size stop lists,
explicit `parallel_tool_calls: true`, and independently invalid tool schemas.
A genuine provider error test must use matching `event: error` and
`data.type: error`; event/type mismatch only tests the framing guard and never
reaches the error arm.

## Resolution references

- `test-06` — 2026-08-09.
