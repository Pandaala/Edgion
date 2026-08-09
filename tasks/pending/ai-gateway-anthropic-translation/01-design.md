# Design

## Solution assessment

| Dimension | Provider-specific translator (chosen) | Generic provider IR now | Require Anthropic-compatible proxy |
|---|---|---|---|
| Change difficulty | High but bounded | Very high | Low |
| Impact scope | Anthropic + shared narrow seams | Every future provider and API shape | Configuration only |
| Safety | Strict allowlist and fixed errors | Larger generic attack surface | Safe but no native support |
| Implementation value | Delivers native Anthropic now | Speculative abstraction | Does not deliver AI-1b |
| Test difficulty | High, protocol-specific | Very high, cross-provider | Low |
| Compatibility | Additive; AI-1a unchanged | Broad AI-1a refactor | No client portability |
| Rollback ability | Remove one provider module | Difficult | Trivial |

### Chosen option

Create an Anthropic-specific translation module behind the existing provider enum. Keep the
current single-service reference and private response-processor pipeline. Extract only narrow
provider-neutral request preparation and header hooks needed by a second provider.

This avoids prematurely freezing a generic intermediate representation before Bedrock and
Gemini constraints are known. It also preserves the already-tested OpenAI passthrough path.

### Rejected options

- A generic multi-provider IR now would mix AI-1b with future provider and routing design,
  while losing provider-specific semantics at the abstraction boundary.
- Requiring an external OpenAI-compatible Anthropic proxy would not provide centralized
  native-provider governance and would leave AI-1b unimplemented.
- Putting AiProxy into operator-facing response plugin stages would duplicate runtime wiring
  and break its thin request-plugin contract.

## Frozen behavior

### Resource and runtime

- Add `AiProvider::Anthropic` with default endpoint `https://api.anthropic.com`.
- Add optional `defaults.maxOutputTokens` (`u32`, greater than zero when present).
- An Anthropic request uses, in priority order:
  1. OpenAI `max_completion_tokens`;
  2. legacy OpenAI `max_tokens`;
  3. `EdgionAIService.spec.defaults.maxOutputTokens`.
- If neither request field nor the service default is present, reject locally with a fixed
  OpenAI-style 400 `missing_required_parameter`. If both request fields are present with
  different values, reject locally. No global token limit is invented.
- One AiProxy continues to reference exactly one service. Picker/fallback semantics stay in
  AI-3.

### Request path and headers

- Anthropic accepts only `POST` requests whose normalized downstream path ends in
  `/chat/completions`; the upstream URI is always `<endpoint-prefix>/v1/messages` and does
  not forward the downstream query.
- Before installing provider headers, remove client `authorization`, `x-api-key`,
  `anthropic-version`, `anthropic-beta`, and `accept-encoding`.
- Install `x-api-key`, `anthropic-version: 2023-06-01`, and
  `content-type: application/json`. Never install Bearer auth for Anthropic.
- Preserve the existing external-jump DNS/SSRF/TLS/SNI/Host behavior and per-service read
  timeout override.
- Request input character estimation always observes the original client JSON, not the
  translated provider JSON.

### Strict supported input matrix

The executable field/type/null/range rules and canonical wire output are frozen in
[`04-wire-contract.md`](04-wire-contract.md). In summary:

- The only supported top-level OpenAI fields are `model`, `messages`, `stream`,
  `stream_options`, `max_completion_tokens`, legacy `max_tokens`, `stop`, `tools`,
  `tool_choice`, `parallel_tool_calls`, `n`, `temperature`, and `top_p`.
- Unknown fields and explicitly unsupported fields fail locally even when null. There is no
  vendor-options escape hatch in this slice.
- `n` is absent/null/`1`. `temperature` and `top_p` are absent/null/exactly `1` and are then
  omitted upstream. Current Claude model families do not share stable support for non-default
  sampling values.
- `system` messages are a contiguous prefix. User/assistant/system/tool content supports only
  strings or exact text parts. `developer` and later system messages fail locally.
- The tool conversation graph is validated: unique declared tools and call IDs, declared
  names, object argument/schema values, immediate consecutive results, exactly one result per
  pending call, and no orphan/missing/duplicate results.
- An assistant message ending the history fails locally because it is an Anthropic prefill,
  which is not stable across the intended model set.
- Legacy function APIs, multimodal content, structured output, log probabilities, penalties,
  seed, prediction, reasoning controls, storage/service tier, and beta/provider extensions
  fail locally.

Every local rejection uses a fixed OpenAI error envelope and `PluginLog::push_err` with a
static classification. Request/provider/body detail is never logged.

### Response header normalization

Extend the private `StreamingResponseProcessor` with two default no-op methods:
`on_response_header(&mut ResponseHeader)` and `changes_body_length() -> bool`. Invoke header
callbacks after raw-status bookkeeping, outlier accounting, and retry arbitration, but before
pending header operations and response plugins.

- Retry matching and the per-attempt upstream log continue to observe the real Anthropic
  status.
- The callback stores the selected success/error body mode inside the Anthropic processor;
  body-stage `PluginSession` deliberately has no response-header access.
- Only HTTP 200 enters success translation. Preserve 400 and 413 as fixed generic client
  errors and 429 as a fixed generic rate-limit error. Map every other status, including
  unexpected 2xx, all 3xx, unlisted 4xx, and all 5xx, to fixed 503.
- A 200 success must have identity/no `Content-Encoding` and the expected media type:
  `text/event-stream` for a streaming request or `application/json` for non-streaming. A
  mismatch becomes fixed 503. Never attempt to transform encoded bytes.
- Strip provider request IDs, `Location`, `Set-Cookie`, `Content-Encoding`, validators tied to
  the original body, and framing headers. Preserve a syntactically valid `Retry-After` only on
  the fixed 429 response. Provider messages and IDs never pass through.
- After the callback mutates the status/header, overwrite `ctx.request_info.status`, reclassify
  the response kind, and make the compression decision from the normalized header.
- For a processor whose `changes_body_length()` is true, remove `Content-Length` at the end of
  `upstream_response_filter` after pending operations and synchronous plugins. This is early
  enough for Pingora H1 to install chunked framing. Remove it again at the end of
  `response_filter` after asynchronous plugins so a later plugin cannot create CL+TE or stale
  H2 length metadata.
- Known non-streaming `Content-Length` above the transformation limit is rejected by the
  header callback as fixed 503 before downstream headers are committed.

### Processor order

For Anthropic, install `AnthropicResponseProcessor` first and
`UsageExtractionProcessor` second. Both share the same `Arc<AiUsageShared>`. The translator
publishes normalized exact provider usage at a valid terminal event even when the client did
not ask for a usage chunk; the existing observer remains responsible for output-character
estimation and harmlessly ignores an already-published exact value. Downstream response-body
plugins see only the client-visible OpenAI representation.

### Non-streaming response conversion

- Accumulate at most 1 MiB of provider input and serialize through a separate capped 1 MiB
  output writer; do not add a response `BodyStorage` or call a full-response API.
- Hold non-streaming chunks until end-of-stream, then parse one Anthropic Message and emit one
  OpenAI Chat Completion JSON body. This is bounded and does not change the upstream read
  pump or backpressure ownership.
- Concatenate text blocks in order. Convert each `tool_use` block to one OpenAI function tool
  call with compact JSON arguments.
- Map `end_turn`, `stop_sequence`, and `pause_turn` to `stop`, `max_tokens` and
  `model_context_window_exceeded` to `length`, `tool_use` to `tool_calls`, and `refusal` to
  `content_filter`. Reject unknown stop reasons and unsupported block types.
- Normalize usage as:
  `prompt_tokens = input_tokens + cache_creation_input_tokens + cache_read_input_tokens`,
  `completion_tokens = output_tokens`, and `total_tokens = prompt_tokens + completion_tokens`.
  All additions saturate.
- On malformed JSON, unsupported content, generated-output overflow, or chunked input overflow
  after headers are committed,
  emit one fixed OpenAI error envelope under the already-committed status and permanently
  discard subsequent provider bytes. This status limitation is recorded as an accepted
  interim risk until response-side termination/header deferral exists; raw Anthropic bytes
  are never used as fallback.

### Streaming response state machine

- Reuse `SseDecoder` with its 64 KiB incomplete-event bound. Never forward raw Anthropic
  events.
- Require exactly one `message_start`; validate/capture identifiers and input usage; emit the initial
  OpenAI chunk with `delta.role = "assistant"`.
- Anthropic's documented lifecycle is serial: at most one block is active. A start index must
  equal the next consecutive block index, and delta/stop indexes must equal the active index.
- `content_block_start(text)`: register the block without output.
- `content_block_start(tool_use)`: assign a consecutive OpenAI tool index and emit id,
  function name, and empty arguments immediately.
- `content_block_delta(text_delta)`: emit `delta.content`.
- `content_block_delta(input_json_delta)`: forward `partial_json` directly as an OpenAI
  function-arguments string delta. Do not accumulate or parse the complete tool input.
- `content_block_stop`: close the active block. Permit at most 128 total blocks.
- One or more `message_delta` events may follow only after all blocks close. Treat every usage
  field as a cumulative latest value and emit a mapped finish reason exactly once.
- Exactly one terminal `message_stop` is required. At it, saturating-add input, cache-creation,
  and cache-read token counts, publish exact usage to `AiUsageShared`, optionally emit an
  OpenAI `choices: []` usage chunk, and emit `[DONE]`.
- Effective usage-chunk emission is client `stream_options.include_usage` OR
  `AiProxy.injectUsageTracking`, preserving AI-1a's default-on client behavior. Exact access-log
  and optional metric usage does not depend on whether that client-visible chunk is emitted.
- Ignore `ping` and unknown top-level stateless event types as required by Anthropic's
  versioning policy. Parse JSON `type` before deciding an event is unknown; a known SSE event
  name/data-type mismatch fails. Unknown content blocks/deltas, malformed order/JSON, repeated
  lifecycle events, premature EOS, more than 128 blocks, decoder degradation, provider `error`,
  or output expansion beyond 1 MiB per callback enters a sealed failure state.
- Sealed failure emits one fixed OpenAI-compatible SSE error event, emits no `[DONE]`, and
  clears any normal output built during the failing callback, records
  `AiResponseTranslationFailed` exactly once in request status, and discards all later upstream
  bytes until EOS. The current pipeline cannot terminate the
  upstream response early; the configured per-read timeout remains the outer bound. This is
  explicit and will be replaced by response-side terminate support in AI-4.
- Message/model/tool IDs and tool names have explicit byte caps in the wire contract. Retained
  state contains only bounded identifiers, token counters, one active block kind/index, next
  block index, block count, and tool ordinal. No prompt,
  completion, credential, provider error message, or complete tool arguments are retained.
- Each `partial_json` value is an opaque provider-owned string. Its event envelope is validated,
  but complete tool-argument JSON is not accumulated or revalidated after it has streamed.

### Metrics and access log

- Add the fixed provider label `anthropic`; do not add a model label.
- The optional/default-off token counter remains the only new metric family. Its fixed maximum
  grows from 8 to 12 series: 3 providers × 2 directions × 2 usage sources.
- Existing access-log fields record the canonical model, `provider=anthropic`, exact normalized
  token totals when usage is present, streaming state, estimation flag, and TTFT.
- Prompts, completions, tool arguments, credentials, and provider error text never enter logs
  or metrics.

## Code change plan

### Resource and schema

- `edgion-resources/src/resources/edgion_ai_service.rs`: provider, endpoint, optional default,
  validation, schema and wire tests.
- `config/crd/edgion-crd/edgion_ai_service_crd.yaml`: provider enum and `maxOutputTokens` schema.
- `edgion-resources/tests/crd_coverage.rs` or a focused provider-enum guard: compare the Rust
  provider enum and committed CRD in both directions because field-presence coverage does not
  detect enum drift.
- `edgion-gateway/src/ai/service_store.rs`: cold-compile the optional output-token default.

### Gateway translation

- `edgion-gateway/src/ai/anthropic/mod.rs`: provider constants and public narrow seams.
- `edgion-gateway/src/ai/anthropic/request.rs`: pure OpenAI request validation/conversion.
- `edgion-gateway/src/ai/anthropic/response.rs`: pure non-streaming conversion, error mapping,
  and stateful bounded SSE processor.
- `edgion-gateway/src/ai/error.rs`: fixed missing/unsupported parameter errors.
- `edgion-gateway/src/plugins/http/ai_proxy/plugin.rs`: provider dispatch, path/header rewrite,
  and processor installation order.
- `edgion-gateway/src/plugins/runtime/traits/streaming_response_processor.rs`: add default
  header/framing declarations.
- `edgion-gateway/src/routes/http/proxy_http/{pg_upstream_response_filter.rs,pg_response_filter.rs}`:
  run header callbacks after retry arbitration and enforce framing at both required phases.
- `edgion-gateway/src/ai/usage.rs` and centralized metrics registry/tests/docs: add the fixed
  Anthropic label and update the series ceiling.

### Tests and documentation

- Unit tests beside request/response conversion for the full supported/rejected matrix,
  byte-split SSE, multi-event chunks, tools, usage, malformed order, bounds, and redaction.
- `edgion-tests/src/server/test_server.rs`: bounded Anthropic fixture plans and observations
  that expose only fixture/generation booleans, never raw headers, bodies, or keys.
- `edgion-tests/src/client/suites/edgion_ai_service/anthropic_provider.rs`: a sibling suite,
  leaving `SingleProvider` unchanged.
- `edgion-tests/integration/conf/EdgionAIService/AnthropicProvider/`: isolated static and
  dynamic fixtures on a new port.
- Dynamic Secret rotation scenario: A to B without controller/gateway restart, admin output
  redacted, neither credential present in logs.
- Update the AI service/AiProxy skills and bilingual user docs, plus metrics inventory.

## Test design

### Unit tests

- Request: model alias, token-limit priority/default/conflict/missing, system placement, text,
  tools/tool results, tool choice, strict unknown/unsupported matrix, and header redaction.
- Non-streaming response: text, multiple tool calls, all stop reasons, exact cache-inclusive
  usage, fixed provider errors, malformed JSON, unsupported block, and 1 MiB boundary.
- SSE: split every event at every byte boundary, many events in one callback, paced text,
  consecutive multi-tool lifecycles, byte-exact JSON argument fragments, cumulative usage, ping/unknown
  event handling, provider error, malformed order, 64 KiB decoder bound, 128-block bound,
  per-callback output cap, and abort before `message_stop`.
- Header hook: retry ordering, status mapping, framing removal, content type, and known-length
  overflow.
- CRD: provider enum and field coverage in both directions.

### Integration tests

Keep all existing AI-1a static and dynamic cases unchanged. Add an isolated Anthropic suite:

1. H1 paced text stream with compression enabled and frame timing assertions.
2. H2 paced text stream.
3. Non-stream request/response/header/path/model/usage conversion.
4. Non-stream tool round trip.
5. Streaming fragmented tool call conversion.
6. Exact/estimated access-log usage and optional metric deltas with no model label.
7. Fixed provider errors plus local unsupported-field rejection that must not reach upstream.
8. Malformed and oversized SSE events that never expose raw Anthropic bytes.
9. Dynamic Anthropic Secret rotation A to B with stable process IDs and redacted admin output.

## Verified assumptions

- The existing response processor may hold bounded non-streaming bytes and replace the EOS
  chunk without changing Pingora's upstream pump ownership. This is the same bounded state class
  already used by non-streaming usage observation and does not add response `BodyStorage`.
- Pingora H1 requires the first framing removal by the end of `upstream_response_filter`; the
  second removal in `response_filter` prevents later CL+TE reintroduction.
- A fixed SSE error followed by sealed drain is the safest implementable interim behavior before
  AI-4 response termination.
- Anthropic's stable Messages API accepts the chosen client-tool and `tool_choice` mappings
  without beta headers.
- Edgion implements no Pingora response-cache callbacks, so an AiProxy response cannot be read
  from a cache and accidentally translated twice.
- `BodyStorageView::publish_arc` enforces the resolved physical request-body cap for
  `overwrite_request_body`, including translated serialization expansion.
- Normal EOS/bodyless callbacks are covered by processor unit tests; client abort without EOS
  remains estimation-only, matching AI-1a.

## Risks and compatibility

- The non-streaming unknown-length overflow cannot change an already-committed HTTP status.
  It remains bounded and never leaks provider bytes, but the body/status pair is imperfect.
- A sealed failed SSE stream cannot stop the upstream read immediately until AI-4. It stays
  bounded and fail-closed at the client schema boundary.
- Unknown provider event types are ignored only at the top-level stateless layer. Unknown content
  semantics fail closed to prevent silently truncated answers.
- The fixed metric series ceiling grows to 12 but remains compile-time bounded and opt-in.
- TTFT remains the first upstream body byte, preserving AI-1a semantics. A provider `ping` may
  therefore precede the first client-visible token; changing the definition is deferred.
- Anthropic text/tool block interleaving is represented by OpenAI's separate `content` and
  `tool_calls` fields, so relative interleaving is not expressible. Block order within each field
  remains stable.
