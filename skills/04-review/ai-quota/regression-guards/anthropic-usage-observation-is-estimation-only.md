---
name: anthropic-usage-observation-is-estimation-only
description: Use when reviewing Anthropic translation or settlement; distinguishes SSE fallback estimation from authoritative bounded unary usage and prohibits a second accounting body copy.
status: fixed
finding_id: gw-18
closed: 2026-08-09
knowledge_type: regression-guards
---

# Anthropic translation owns usage; SSE alone owns fallback estimation

## Conclusion

`AnthropicResponseProcessor` owns provider parsing, exact usage publication,
quota accumulation, and mid-stream quota termination. The generic
`UsageExtractionProcessor` therefore must not buffer or parse the translated
OpenAI-shaped body again.

For SSE, the translator also owns translated `output_chars`, the bounded fallback
for eligible incomplete streams. Unary conversion publishes checked provider usage
only after full document validation; invalid/incomplete JSON and fixed errors never
produce character estimates. Valid usage survives later output overflow, local
trailer failure, delivery cancellation and terminal cleanup. Dispatch installs no
generic observer after the translator.
This supersedes the historical estimation-only observer described by gw-18:
that observer was needed while it was the sole character-counter owner, but
ownership has since moved into the translator.

The dispatch contract is explicitly selected by AiProxy API/conversion/observation:

- Native OpenAI Chat installs usage extraction only with `usageObservation: provider`.
- Explicit `chatToAnthropic` installs its translator, never a second usage observer.
- Observation defaults off for both paths. Converter-intrinsic client usage still works,
  while `AiUsageShared` suppresses numeric publication and accounting stays `notObserved`.
- Native off with explicit semantic Guardrail installs the canonical semantic-only source,
  without character counting, JSON usage observation or numeric accounting.

Do not install a second SSE decoder or retain the translated body for accounting.
The bounded unary remediation
is implemented with selective preflight, checked usage, borrowed serialization and
shared admission/output ownership; fresh workspace/shared-topology verification
and final independent Astra code/evidence review passed. The historical task
attribution includes the unrelated global WAF documentation exception. Its path
was `tasks/todo/ai-gateway-product-completion/03-bounded-anthropic-json-conversion.md`;
the task file is absent from the current tree, so its historical evidence is not
available here for independent inspection. The converter-only exception in
[AGENTS.md](../../../../AGENTS.md#http-body-handling-invariants) authorizes no second
usage-only response copy. Document validation, actual source EOS and client
outcome remain distinct: Released does not synthesize source EOS or delivery success.

## Review Guidance

Do not reintroduce the generic processor after Anthropic translation or remove
the SSE translator's character counter. Verify reported usage and valid estimated
SSE disconnect/gateway-cut paths. For unary, verify no error-body estimates and
retention of valid checked usage after local output/delivery failure. Unsupported raw content encoding must be marked
unavailable before the translator rebuilds headers; translated error-envelope
characters must not restore estimated accounting.

Re-evaluate when character ownership, encoding eligibility, or termination
ordering changes. The translator's `!sink.is_terminated()` gate still excludes
the terminating chunk from its fallback counter; preserve the accepted bias in
`gateway-terminated-stream-settles-its-reservation.md`.

## Regression Coverage

- Mirrored dispatch tests verify translator-only processing and encoding capture.
- Existing Anthropic usage/quota and gateway-terminated settlement tests.
- Fresh Task 03 AI units: 374 passed; unary lifecycle filter: 12 passed, including
  canceled driver callbacks. Unary output-overflow/cleanup tests retain known usage.
- Real-wire unary valid documents with rejected trailers retain known 7/11 usage;
  malformed/reset/timeout paths stay unknown. This is translator ownership evidence,
  not SDK/provider conformance or a process memory benchmark.

## Resolution references

- `gw-18` — 2026-08-09.

## Native Messages distinction

Native anthropicMessages does not install the Chat translator. Its separate bounded native
observer is the sole native provider observer when requested, with reported-only consumption
and no character fallback. This does not authorize installing an observer after conversion.
See [native Messages ownership](../../ai-routing/regression-guards/native-messages-preservation-and-accounting.md).
