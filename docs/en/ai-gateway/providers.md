# Providers

Clients always send OpenAI Chat Completions requests. Edgion maps each logical model to the
selected backend's provider model independently.

## Supported providers

| `provider` | Protocol behavior | Default endpoint |
|------------|-------------------|------------------|
| `OpenAI` | Passthrough | `https://api.openai.com` |
| `OpenAICompatible` | Passthrough | **Required** `endpoint` |
| `Anthropic` | Streaming translation to native Messages API | `https://api.anthropic.com` |

Bedrock, Vertex, multimodal, beta-provider, and non-chat APIs are out of scope for this surface.

## OpenAI and OpenAI-compatible

Requests and responses stay in OpenAI Chat Completions form. Optional path prefixes on `endpoint`
are supported. Set `allowInsecureHttp: true` only on controlled development networks.

`onUnknownModel: passthrough` is limited to OpenAI-compatible candidates with `Weighted` or
`Priority` selection. Anthropic translation always requires a catalog match.

## Anthropic translation

For `provider: Anthropic`, Edgion rewrites the upstream call to the native Messages endpoint and
incrementally translates both JSON and SSE responses. Set an output bound through
`max_completion_tokens`, legacy `max_tokens`, or `defaults.maxOutputTokens`. Unsupported fields
fail locally rather than being silently removed. The main upstream response remains streaming, and
raw provider error bodies are never forwarded to clients.

## Credential installation

Edgion removes client provider-authentication headers and installs the selected Secret-backed
credential for each attempt. Applications never receive provider keys.
