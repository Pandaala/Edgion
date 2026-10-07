# Providers

Managed AiProxy entries support OpenAI Chat Completions, native OpenAI Responses and native Claude Messages. Edgion maps the logical model to
the selected backend's provider model. Clients choose the model; an explicit model rewrite or
quota downgrade policy can change that choice.

## Current support

| Client operation | Backend | Current behavior |
|------------------|---------|------------------|
| `POST /v1/chat/completions` | `OpenAI` / `OpenAICompatible` | Chat JSON or SSE; provider fields remain, subject to explicitly configured request mutation |
| `POST /v1/chat/completions` | `Anthropic` | Chat-to-Messages requests; JSON or SSE translated back to Chat, limited to the subset below |
| `GET /v1/models` | Chat API route-reachable model catalog | Locally generated catalog; HTTPRoute must admit GET and match this path |
| `POST /v1/responses` | `OpenAI` | Native Responses JSON or SSE; explicit model required |
| `POST /v1/messages` | `Anthropic` | Native Messages JSON or SSE; explicit model and max_tokens required |
| Gemini native, Bedrock | Any managed backend | Not yet supported; deferred scope |

The configured `api: openaiChat` is authoritative. Ingress accepts exact effective POST
`/v1/chat/completions` and local GET `/v1/models`; custom paths need an earlier URLRewrite.
Other managed operations return fixed `404 unsupported_endpoint`. `api: openaiResponses` instead admits exact POST `/v1/responses` for `OpenAI` only.
`api: anthropicMessages` admits exact POST `/v1/messages` for `Anthropic` only.
An ordinary HTTPRoute to a Service can separately proxy opaque HTTP traffic without managed AI semantics.

## Supported providers

| `provider` | Protocol behavior | Default endpoint |
|------------|-------------------|------------------|
| `OpenAI` | Native Chat or Responses requests and responses | `https://api.openai.com` |
| `OpenAICompatible` | Chat requests and responses | **Required** `endpoint` |
| `Anthropic` | Native Messages or explicitly selected Chat-to-Messages conversion | `https://api.anthropic.com` |

Gemini, Bedrock and Vertex remain outside current managed support.

## OpenAI and OpenAI-compatible

Requests and responses stay in Chat Completions form. Optional path prefixes on `endpoint` are
supported; set `allowInsecureHttp: true` only on controlled development networks. Provider
extensions are interpreted by the backend, without a promise that the gateway understands them.
Model mapping and `injectUsageTracking` can mutate requests, and quota Reject can terminate SSE.
Do not assume byte-for-byte forwarding with these policies enabled.

`onUnknownModel: passthrough` requires `Weighted` or `Priority` and all valid positive-weight
candidates to be `OpenAI` or `OpenAICompatible`. An explicit model rewrite still requires a
canonical catalog name. Anthropic conversion always requires a catalog match.

## Anthropic translation

Use `api: openaiChat` with `conversion: chatToAnthropic` and exclusively Anthropic candidates
to select the existing Chat-to-Messages converter. Native Chat entries omit conversion and use only
OpenAI/OpenAICompatible candidates. Native Messages uses Anthropic candidates. The subset covers text messages, declared
function tools, tool calls/results, `tool_choice`, `parallel_tool_calls`, and `stop`. `n`,
`temperature`, and `top_p` accept only omission, null, or the neutral value 1, not arbitrary
sampling controls. System messages must precede the conversation, which must end with a user
message or tool result; developer role is unsupported. Multimodal content, structured output, unknown fields, and unsupported
combinations fail locally rather than being silently removed. Resolve the required output-token
value through `max_completion_tokens`, legacy `max_tokens`, or `defaults.maxOutputTokens`.
The default is a fallback, not an enforced ceiling.

SSE conversion is incremental over bounded events. Non-streaming JSON retains one in-memory
document under input/output byte, parsing-complexity, per-exchange reservation, and shared process
budgets. Validation and serialization finish before final headers or body commit. This is confined
to the selected converter, not a general response cache; ordinary HTTP, OpenAI responses, and SSE
remain streaming. See [Credentials and Limits](./credentials-and-limits.md#unary-conversion-resource-limits)
for resource configuration and defaults.

Invalid, truncated, or oversized conversion returns fixed Chat error `502 upstream_response_invalid`
before commitment; timeout returns fixed `504 upstream_timeout`. Admission capacity failure returns
fixed `503 service_unavailable` before provider dispatch and cannot be bypassed through failover.
Raw Anthropic error bodies are never forwarded; true provider errors and explicit plugin denials
retain their existing error handling.

## Credential installation

Edgion removes client provider-authentication headers and installs the selected Secret-backed
credential for each attempt. Applications do not need provider keys; configure ordinary
authentication plugins separately for client authentication.

## Native Responses subset

Set `api: openaiResponses`; omit `conversion` and leave `injectUsageTracking: false`.
Observation defaults off. Set `usageObservation: provider` to observe native usage without
injecting Chat's `stream_options.include_usage`. Native `stream_options` and provider extensions
pass through unchanged. The model must be explicit for managed routing, even where the provider
would accept omission. Only POST `/v1/responses` is managed; GET/list/retrieve/delete/cancel,
background creation (`background: true`), WebSocket and Responses-to-other-provider conversion
are excluded. Hosted tools run at the provider. Edgion preserves input/output items, tool fields,
reasoning and opaque identifiers; it does not normalize them into a conversation representation.

A non-null `previous_response_id` or `conversation` requires exactly one declared backend slot
and one declared credential binding. Unresolved credentials and zero-weight backend slots still
count. Credential weight zero is invalid configuration. The two continuation fields are mutually
exclusive. Continuation rejects aliases that change the provider model, any configured `modelRoutes`,
quota Downgrade and retries; no automatic principal/backend fallback or affinity registry is added.
Stateless requests retain normal selection. Rotating a Secret across provider principals is an
operator responsibility; retaining the binding name does not prove account affinity.

AiGuard and Guardrail currently understand Chat semantics. Enabled combinations with Responses
are rejected in the same resource; actual invocation across scopes is fenced at runtime, including
an AiGuard invoked before URLRewrite. Disabled, condition-skipped and policy-bypassed guards do
not latch the runtime fence. Native guard capabilities are a separate future task.

See [Responses API](https://developers.openai.com/api/reference/resources/responses/) and
[typed streaming events](https://developers.openai.com/api/reference/resources/responses/streaming-events).

Managed credential installation also removes OpenAI-Organization and OpenAI-Project, including
backend header modifier values. RequestId cannot target these reserved headers and later restore
them. These are provider principal selectors, not arbitrary native extension headers. Use a
project-scoped key and maintain a fixed operator-controlled principal; selector overrides are not
supported by the current managed contract. Ordinary opaque HTTP proxying is unaffected.

## Native Messages subset

Set `api: anthropicMessages`; omit `conversion` and `injectUsageTracking`. Use
`provider: Anthropic` on EdgionBackend. Only POST `/v1/messages` is managed;
count_tokens, files, batches and other management endpoints are outside this entry.
Request model and nonnegative integer max_tokens are required; explicit zero is
preserved for cache prewarming. Native JSON/SSE, tools, signatures, cache controls
and opaque extensions pass through. Stateless explicit model mapping changes only
the model token.

All effective anthropic-beta values and the query are preserved. Missing
anthropic-version defaults to `2023-06-01`; provider observation is certified for
that version only. Other versions pass through off, or become sticky unknown with
optional observation. Required accounting rejects an unsupported version before
dispatch. Client authentication and workspace selectors are removed, then the
Secret key is installed. Use a key owned by one workspace; effective
anthropic-user-profile-id is unsupported. Late RequestId/RemoteIp rewrites cannot
override managed identity or certified protocol headers.

Container reuse, thinking/redacted-thinking history, container uploads and file
references (including nested tool_result content) require one raw backend slot,
one declared credential, unchanged provider model and no modelRoutes, downgrade
or retry. Unresolved credentials and zero-weight backend slots still count.
Secret rotation must preserve the relevant account/workspace/model; Edgion does
not prove object authorization or track affinity. Unknown beta identifiers remain
opaque and have no portability guarantee.

AiGuard/Guardrail combinations are rejected at validation and actual invocation.
Provider observation uses bounded selective parsing without a response cache.
Input consumption includes input_tokens plus cache_creation_input_tokens plus
cache_read_input_tokens; breakdowns are not counted again. Late SSE components
replace cumulative watermarks. Start placeholders alone do not establish final
usage: message_delta output and valid stop metadata/message_stop are required.
Missing/malformed/oversized/encoded metadata, partial streams and provider error
produce unknown accounting; passive observation preserves response bytes.
Active quota Reject emits native `event: error` and closes without a Chat DONE
sentinel. Normal tool_use, pause_turn, max_tokens and refusal are valid stops.

Known compaction/advisor/provider fallback inference remains opaque off and sticky
unknown with observation. Accounting-required requests reject recognized
composite inference before dispatch. Response iteration metadata is bounded;
only same-model message summaries qualify, without adding their subtotals again.
Configured rates estimate token cost; they do not cover every tool fee, tier,
mixed cache TTL or the complete provider invoice.

See [Messages](https://platform.claude.com/docs/en/api/messages/create) and
[streaming](https://platform.claude.com/docs/en/build-with-claude/streaming).
