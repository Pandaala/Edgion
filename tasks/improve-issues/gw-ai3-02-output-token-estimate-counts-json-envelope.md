# gw-ai3-02: Output-token estimation counts every character of the JSON/SSE envelope, overcharging estimated usage by ~30-60x

- **Category**: design-error
- **Severity**: P1 (billing/quota correctness)
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/ai/accounting/usage.rs:770` (`UsageExtractionProcessor::new_with_quota` enables `sse.enable_data_char_counting()`)
  - `edgion-gateway/src/ai/accounting/usage.rs:889` (`self.shared.add_output_chars(self.sse.take_data_chars())` per chunk)
  - `edgion-gateway/src/ai/accounting/usage.rs:908` (non-streaming: `Utf8CharCounter` over the whole response body)
  - `edgion-gateway/src/ai/accounting/usage.rs:502-560` (`accounting_snapshot`: `output_tokens = ceil(output_chars / 4)` when provider usage is absent)
  - `edgion-gateway/src/ai/conversion/chat_to_anthropic/response.rs:698-712, 852-861` (translator counts the full encoded chunk JSON as output chars)
  - `edgion-gateway/src/sse/mod.rs:674-720` (`DataCharObserver` counts all `data:` value characters)
  - Consumers: `ai/runtime/attempt_runtime.rs:177-191` (`final_charged_tokens`), `:570-620` (`commit_ledger` → business quota, cost, LowestUsage); `ai/accounting/finalize.rs:113-160`

## Summary

When provider usage is missing, the gateway falls back to `output_tokens = ceil(output_chars / 4)`.
But `output_chars` is not the generated text — it is the character count of the entire SSE `data:`
payload (every chunk's `id`, `object`, `created`, `model`, `system_fingerprint`, `choices`,
`index`, `delta`, `logprobs`, `finish_reason` …) or, for non-streaming bodies, the whole JSON
document. A typical OpenAI streaming chunk carries one token of content inside ~200-250 characters
of envelope, so the estimate is ~50x the real output. The estimate is then charged to the tenant's
business `tokenQuota`, the access-log cost, the credential reconciliation and the LowestUsage
telemetry.

## Current behavior (evidence)

- Chat SSE: `new_with_quota` calls `sse.enable_data_char_counting()`; each callback adds
  `take_data_chars()` to `AiUsageShared::output_chars`. `DataCharObserver` (`sse/mod.rs:674`) counts
  UTF-8 scalars in every `data:` field value, i.e. the full JSON event, plus `[DONE]`.
- Translated Anthropic SSE: `push_json_event` adds the char count of
  `encoded_record[6..len-2]` — the full OpenAI-shaped chunk object — to `pending_output_chars`, which
  becomes `add_output_chars` (`response.rs:698-712, 852-861`).
- Non-streaming Chat: `Utf8CharCounter::feed(chunk)` over the whole response body.
- `accounting_snapshot(..., include_output = true)` uses
  `ceil_chars_div_four(self.output_chars())` whenever `reported_usage` is `None` and observation is
  still "valid" (`usage.rs:529-547`).

When the fallback is reached in practice (documented in
`skills/04-review/ai-quota/regression-guards/gateway-terminated-stream-settles-its-reservation.md:59`:
"A valid observed SSE prefix can still produce estimated ledger/business usage on client
disconnect"):

- the client cancels a stream (user presses "stop" — very common in chat UIs) before the final
  `include_usage` chunk;
- the gateway terminates the stream for quota;
- an OpenAI-compatible backend ignores `stream_options.include_usage` (several self-hosted servers
  and older Azure API versions).

Worked example — 1,000 generated tokens, 1 token per chunk, ~230-char chunk:
`output_chars ≈ 230,000` → estimated `output_tokens ≈ 57,500` (57x). For a request with a 2k-token
prompt the business quota is charged ~59.5k instead of ~3k.

## Industry comparison

- LiteLLM / Kong AI Proxy / Envoy AI Gateway estimate missing usage by tokenizing (or char-counting)
  the *extracted content* (`delta.content`, `tool_calls[].function.arguments`), never the transport
  envelope.
- Edgion already extracts exactly those spans for the semantic guard
  (`ai/protocols/openai/canonical_sse.rs:339-502` computes content / reasoning / tool-argument token
  ranges per event), so the right input is available on the same pass.

## Impact

- Tenants are over-charged against `tokenQuota` by an order of magnitude on every cancelled or
  usage-less stream, causing spurious 429s / downgrades and wrong cost reports.
- LowestUsage selection is skewed toward backends that happen to see fewer cancellations.
- The credential TPM reconciliation is inflated in the same way.

## Suggested direction

1. Count only semantic output: in the Chat SSE path, sum the decoded lengths of
   `delta.content`, `delta.reasoning_content` and `tool_calls[].function.arguments` that
   `observe_canonical_block` already locates; in the Anthropic translator, count the provider
   `text_delta.text` / `input_json_delta.partial_json` lengths instead of the re-encoded chunk.
2. For non-streaming bodies, do not estimate from the raw document; if usage is missing, either
   count `choices[].message.content` spans or mark usage `Unknown` (as reported-only APIs already do).
3. Keep the `chars / 4` heuristic but document it as applying to content characters only.

## Notes

- Checked `skills/04-review/ai-quota/regression-guards/*` (sse-degradation, gateway-terminated,
  anthropic-usage-observation-is-estimation-only) and `ai-routing/streaming-usage-is-a-monotonic-
  cumulative-snapshot.md`: they accept *that* a character estimate is used, but none discusses or
  accepts that the estimate's input is the transport envelope.
- Related but distinct: `gw-ai-01` (estimates charged for error responses). This finding is about
  the magnitude of every estimate, including legitimate successful/cancelled streams.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P1 unchanged.
- `ai/accounting/usage.rs:770` enables `sse.enable_data_char_counting()`. `:889` adds `take_data_chars()` per chunk. `:908` feeds the whole non-streaming body into `Utf8CharCounter`.
- `sse/mod.rs:674-720` (`DataCharObserver`) counts every character of each `data:` value, which is the full JSON event and not the extracted content.
- `usage.rs:529-547` uses `ceil_chars_div_four(self.output_chars())` whenever reported usage is absent.
- No `skills/04-review` page accepts envelope characters as the estimation input; existing pages accept only that an estimate exists. The finding is distinct from gw-ai-01, which is about charging error attempts.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P1 unchanged.
- `accounting/usage.rs:770` still enables `sse.enable_data_char_counting()`, `:889` still adds `take_data_chars()`, and `:908` still feeds the whole non-streaming body into `Utf8CharCounter`; the only `usage.rs` diff is `fetch_update`→`try_update` and a comment rewrite.
- `usage.rs:530-536` still estimates `output_tokens = ceil_chars_div_four(self.output_chars())` when reported usage is absent.
- `conversion/chat_to_anthropic/response.rs:701-708` still counts the full re-encoded chunk JSON, and `:852-861` adds it via `add_output_chars`. `sse/mod.rs` (`DataCharObserver`, `:674`) is unchanged.
