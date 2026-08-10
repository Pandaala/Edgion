---
name: anthropic-usage-observation-is-estimation-only
description: Use when reviewing Anthropic response processors, usage extraction, or AI settlement; explains why translated Anthropic output keeps character estimation but must not be buffered and parsed again.
status: fixed
finding_id: gw-18
closed: 2026-08-09
---

# Anthropic translated output uses estimation-only observation — FIXED

## Conclusion

`AnthropicResponseProcessor` owns provider parsing, exact usage publication,
quota accumulation, and mid-stream quota termination. The generic
`UsageExtractionProcessor` therefore must not buffer or parse the translated
OpenAI-shaped body again.

It cannot be removed entirely. It remains the sole writer of translated
`output_chars`, which is the bounded fallback used when a stream terminates or
fails before Anthropic publishes final usage. Removing it would reconcile those
attempts as if they produced zero output tokens.

The dispatch contract is consequently provider-specific:

- OpenAI and OpenAI-compatible providers install full usage extraction.
- Anthropic installs its translator followed by an estimation-only observer.

Estimation-only mode retains UTF-8/SSE character counting and EOS flushing, but
does not retain a non-streaming body, run a second SSE decoder, parse JSON,
publish reported usage, diagnose SSE degradation, or enforce quota.

## Review Guidance

Do not reintroduce the full generic processor after Anthropic translation, and
do not optimize it away completely. Either change loses one side of the
contract: redundant hot-path memory/CPU in the first case, or under-accounted
incomplete and gateway-terminated attempts in the second.

Re-evaluate only if the Anthropic translator becomes the owner of translated
`output_chars` as well as exact usage. If that happens, remove the
estimation-only observer and update the accepted one-chunk undercount rationale
in `gateway-terminated-stream-settles-its-reservation.md`.

## Regression Coverage

- `ai::dispatch::tests::anthropic_uses_estimation_only_after_provider_translation`
- `ai::usage::tests::estimation_only_counts_without_buffering_or_parsing`
- Existing Anthropic usage/quota and gateway-terminated settlement tests

