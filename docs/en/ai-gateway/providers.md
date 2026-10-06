# Providers

The current managed AiProxy entry uses OpenAI Chat Completions. Edgion maps the logical model to
the selected backend's provider model. Clients choose the model; an explicit model rewrite or
quota downgrade policy can change that choice.

## Current support

| Client operation | Backend | Current behavior |
|------------------|---------|------------------|
| `POST /v1/chat/completions` | `OpenAI` / `OpenAICompatible` | Chat JSON or SSE; provider fields remain, subject to explicitly configured request mutation |
| `POST /v1/chat/completions` | `Anthropic` | Chat-to-Messages requests; JSON or SSE translated back to Chat, limited to the subset below |
| `GET /v1/models` | Route-reachable model catalog | Locally generated catalog; HTTPRoute must admit GET and match this path |
| `POST /v1/responses`, native `POST /v1/messages` | Any managed backend | Not yet supported |
| Gemini native, Bedrock | Any managed backend | Not yet supported; deferred scope |

Ingress identifies operations by method and the `/chat/completions` or `/models` path suffix.
Different prefixes work; arbitrarily renamed paths need an earlier rewrite to a supported suffix.
Other managed operations return fixed `404 unsupported_endpoint`.
`api`, `conversion`, and `usageObservation` are not AiProxy configuration fields yet and cannot
be used in current configuration. An ordinary HTTPRoute to a Service can separately proxy opaque
HTTP traffic. It does not provide AI credential, model, or quota semantics and does not bypass
the requirement to pair EdgionBackend with AiProxy.

## Supported providers

| `provider` | Protocol behavior | Default endpoint |
|------------|-------------------|------------------|
| `OpenAI` | Chat requests and responses | `https://api.openai.com` |
| `OpenAICompatible` | Chat requests and responses | **Required** `endpoint` |
| `Anthropic` | Chat-to-Messages conversion; JSON and SSE responses translated back to Chat | `https://api.anthropic.com` |

Gemini, Bedrock, Vertex, and other non-Chat APIs are outside current managed support.

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

Selecting `provider: Anthropic` automatically selects the current Chat-to-Messages converter;
no AiProxy `conversion` field is required or supported. The subset covers text messages, declared
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
