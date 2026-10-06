---
name: anthropic-usage-observation-is-estimation-only
description: Use when reviewing Anthropic response processors, usage extraction, or AI settlement; explains why translated Anthropic output keeps character estimation but must not be buffered and parsed again.
status: fixed
finding_id: gw-18
closed: 2026-08-09
knowledge_type: regression-guards
---

# Anthropic translation owns usage and fallback estimation

## Conclusion

`AnthropicResponseProcessor` owns provider parsing, exact usage publication,
quota accumulation, and mid-stream quota termination. The generic
`UsageExtractionProcessor` therefore must not buffer or parse the translated
OpenAI-shaped body again.

The translator also owns translated `output_chars`, the bounded fallback for
eligible incomplete streams. Dispatch installs no generic observer after it.
This supersedes the historical estimation-only observer described by gw-18:
that observer was needed while it was the sole character-counter owner, but
ownership has since moved into the translator.

The dispatch contract is provider-specific:

- OpenAI and OpenAI-compatible providers install full usage extraction.
- Anthropic installs its translator, which owns exact usage and estimation.

Do not install a second SSE decoder or retain the translated body for accounting.
The existing non-streaming translation accumulator is a separate pending
conversion task; this guard does not endorse full-response buffering.

## Review Guidance

Do not reintroduce the generic processor after Anthropic translation or remove
the translator's character counter. Verify reported usage and valid estimated
disconnect/gateway-cut paths. Unsupported raw content encoding must be marked
unavailable before the translator rebuilds headers; translated error-envelope
characters must not restore estimated accounting.

Re-evaluate when character ownership, encoding eligibility, or termination
ordering changes. The translator's `!sink.is_terminated()` gate still excludes
the terminating chunk from its fallback counter; preserve the accepted bias in
`gateway-terminated-stream-settles-its-reservation.md`.

## Regression Coverage

- Mirrored dispatch tests verify translator-only processing and encoding capture.
- Existing Anthropic usage/quota and gateway-terminated settlement tests.

## Resolution references

- `gw-18` — 2026-08-09.
