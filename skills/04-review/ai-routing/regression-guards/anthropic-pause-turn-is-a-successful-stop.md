---
knowledge_type: regression-guards
---

# Anthropic `pause_turn` is a successful stop

Anthropic may return `stop_reason: "pause_turn"` when a long-running server
tool pauses a turn. This is a successful provider response with usable content,
not a translation error. OpenAI has no equivalent continuation signal, so the
closest compatible finish reason is `stop`.

Keep the streaming and non-streaming translators on the shared stop-reason
mapping. Known Anthropic reasons must be enumerated there; unknown values still
fail closed so provider wire changes cannot silently acquire incorrect meaning.
Tests should keep one genuinely unknown token to preserve that rejection path.

## Resolution references

- `gw-21` — 2026-08-09.
