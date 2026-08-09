# AI Gateway Anthropic Translation

- **Status**: implemented and verified
- **Roadmap item**: AI-1b, first provider-native translation slice
- **Depends on**: completed AI-0 response pipeline and AI-1a single-provider passthrough
- **Canonical design**: [`01-design.md`](01-design.md)

## Goal

Allow an OpenAI Chat Completions client to call one Anthropic-native
`EdgionAIService` through the existing `AiProxy` plugin. Edgion translates request and
response schemas, injects the provider credential, preserves incremental SSE delivery,
and feeds the existing usage/access-log pipeline with normalized OpenAI usage.

## Scope

- One `AiProxy` entry references one `EdgionAIService`.
- Add the native `Anthropic` provider and the Messages API.
- Translate text chat, client-defined function tools, tool calls/results, non-streaming
  responses, SSE responses, stop reasons, errors, and usage.
- Keep memory bounded and never expose provider credentials or raw provider error detail.
- Preserve all AI-1a OpenAI/OpenAI-compatible passthrough behavior and tests.

## Explicitly out of scope

- Bedrock eventstream and AWS SigV4.
- Azure OpenAI, Google Vertex, workload identity, or token exchange.
- Multi-service model picking, fallback, retries, credential pools, or capability routing.
- Multimodal input, reasoning/thinking blocks, citations, prompt caching controls,
  structured output, provider beta features, and non-chat endpoints.
- A generic vendor-options escape hatch. Unsupported fields fail locally instead of being
  silently discarded.

## Progress

- [x] Context and requirement analysis
- [x] Initial code-level design
- [x] Independent design review and revision
- [x] Implementation and direct unit tests
- [x] Independent code review
- [x] Integration coverage
- [x] Required checks and wrap-up
