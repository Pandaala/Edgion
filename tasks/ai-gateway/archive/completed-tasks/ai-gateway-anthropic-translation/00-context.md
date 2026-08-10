# Context

## Requirement summary

- **Problem to solve**: AI-1a can proxy only OpenAI-shaped upstreams. A native Anthropic
  Messages endpoint uses different authentication, paths, request/response schemas, SSE
  events, tool representation, stop reasons, errors, and usage fields.
- **Expected behavior**: an OpenAI Chat Completions client can use an Anthropic-native
  `EdgionAIService` without carrying a provider key and sees an OpenAI-compatible response
  for both non-streaming and SSE requests.
- **Explicitly out of scope**: cloud-provider authentication, Bedrock binary eventstream,
  multi-service selection, multimodal and provider-specific beta features, and non-chat
  APIs.

## Initial impact assessment

- **Difficulty**: high. Request conversion is bounded whole-body JSON; response conversion
  includes a stateful SSE transducer and response-header normalization.
- **Impact scope**: resource schema/CRD, gateway runtime compilation, AiProxy request path,
  private streaming processors, usage labels, test server, static integration suites,
  dynamic Secret-rotation tests, and user/developer documentation.
- **Safety**: fail closed on unsupported or malformed input; strip all client-controlled
  provider auth/version headers; emit only fixed provider-error categories.
- **Compatibility**: existing OpenAI and OpenAI-compatible paths remain byte-passthrough.
  `Anthropic` is additive. Edgion is pre-release, so no legacy schema shim is required.
- **Rollback**: remove the additive provider enum/default field and translator registration;
  AI-1a remains independently covered.
- **Test difficulty**: high. Chunk-boundary, pacing, tool-call delta, bounded-memory,
  redaction, metric, access-log, and hot-reload behavior all require explicit coverage.

## Existing seams

- `AiProxy::run_request_with_service` already resolves one immutable service snapshot,
  strips client Authorization, rewrites the request body, installs an external jump, and
  attaches usage state.
- `StreamingResponseProcessor` mutates upstream response chunks in installation order
  before read-only response-body plugins.
- `SseDecoder` incrementally frames SSE with a 64 KiB incomplete-event bound.
- `UsageExtractionProcessor` consumes OpenAI-shaped usage and uses a bounded 1 MiB
  non-streaming observer.
- `pg_upstream_response_filter` owns response classification, retry arbitration, header
  mutation, and compression setup. Body processors cannot safely repair headers later.

## Provider contract verified on 2026-08-03

- Native endpoint: `POST /v1/messages`.
- Required headers: `x-api-key`, `anthropic-version: 2023-06-01`, and JSON content type.
- `max_tokens` is required by the Messages API.
- Streaming order is `message_start`, content-block lifecycle events,
  `message_delta`, and `message_stop`; `ping` may occur anywhere.
- `message_delta.usage` is cumulative. Tool input deltas are partial JSON strings.
- A stream may carry an error event after HTTP 200, and future event types may be added.

Official sources:

- <https://platform.claude.com/docs/en/api/messages/create>
- <https://platform.claude.com/docs/en/build-with-claude/streaming>
- <https://platform.claude.com/docs/en/api/errors>
- <https://platform.claude.com/docs/en/api/versioning>
- <https://platform.claude.com/docs/en/manage-claude/authentication>

## Related skills

- `skills/00-design-rules.md`
- `skills/01-architecture/02-gateway/12-edgion-plugin-dev.md`
- `skills/01-architecture/05-resources/SKILL.md`
- `skills/03-coding/observability/00-access-log.md`
- `skills/05-testing/01-integration-testing.md`
- `skills/11-doc/SKILL.md`
- `skills/07-tasks/new_feature_work_flow.skill`
