# Anthropic Translation Wire Contract

This file is the executable protocol contract for AI-1b. Unit and integration tests use these
rules as golden behavior.

## Top-level request fields

| Field | Accepted input | Null policy | Anthropic output / behavior |
|---|---|---|---|
| `model` | non-empty string present in the selected service catalog | reject | canonical model string |
| `messages` | non-empty array of strict message objects | reject | converted `messages`; leading system prefix moves to top-level `system` |
| `stream` | boolean | null means `false` | boolean |
| `stream_options` | strict object containing only boolean `include_usage` | null means absent | not forwarded; controls client usage chunk |
| `max_completion_tokens` | integer `1..=u32::MAX` | null means absent | `max_tokens` |
| `max_tokens` | integer `1..=u32::MAX` | null means absent | `max_tokens` |
| `stop` | non-empty string or array of 1..=4 non-empty strings | null means absent | `stop_sequences` |
| `tools` | non-empty array of strict function tools | null means absent | Anthropic client tools |
| `tool_choice` | `auto`, `required`, `none`, or strict named-function object | null means absent | `auto`, `any`, `none`, or `tool` |
| `parallel_tool_calls` | boolean; legal only with tools | null means absent/true | false sets `disable_parallel_tool_use: true` unless tool choice is `none` |
| `n` | exactly integer `1` | null means `1` | omitted |
| `temperature` | exactly numeric `1` | null means absent | omitted |
| `top_p` | exactly numeric `1` | null means absent | omitted |

Both token fields may be present only when equal. If both are absent, use the service's optional
`defaults.maxOutputTokens`; if that too is absent, return 400 `missing_required_parameter`.
`stream_options` is legal only when effective `stream` is true. The effective downstream usage
chunk flag is `include_usage || injectUsageTracking`.

Every other top-level field is rejected, including when its value is null. This includes legacy
`functions`/`function_call`, `response_format`, log-probability fields, penalties, `seed`,
multimodal/audio fields, `prediction`, `store`, `service_tier`, `reasoning_effort`, `user`,
`metadata`, and provider-specific extensions.

## Messages and text content

Every message object rejects unknown keys.

| Role | Required/allowed fields | Rules |
|---|---|---|
| `system` | `role`, `content` | Only in one contiguous prefix. Content is a non-empty string or non-empty exact `{type:"text",text:string}` part array. Each becomes a top-level Anthropic system text block. |
| `user` | `role`, `content` | Same text forms. Converts to Anthropic user text blocks. |
| `assistant` | `role`, optional `content`, optional `tool_calls` | Content is null, string, or exact text parts. At least non-empty content or a non-empty tool-call list is required. A final assistant message is rejected. |
| `tool` | `role`, `tool_call_id`, `content` | ID is non-empty and content uses the text forms. Must immediately resolve one pending assistant tool call. Consecutive tool messages are grouped into one Anthropic user message. |

`developer`, multimodal part types, message `name`, refusal/audio/annotation fields, and every
other role or member are rejected.

Tool-call IDs are unique and at most 512 UTF-8 bytes. After an assistant emits tool calls, only
an immediate consecutive group of tool messages may follow; every pending ID must be resolved
exactly once before another user/assistant message. Orphan, duplicate, missing, and unknown IDs
are rejected. The request must end in an Anthropic user turn (an ordinary user message or a
complete grouped tool-result turn).

## Tool definitions and choices

- A tool object is exactly `{type:"function",function:{...}}`.
- A function contains `name`, optional `description`, and `parameters`. Unknown members,
  including OpenAI `strict`, are rejected.
- Names are unique and match `^[a-zA-Z0-9_-]{1,64}$`; description is a string when present;
  parameters is a JSON object.
- Assistant tool calls are exactly `{id,type:"function",function:{name,arguments}}`. The name
  must reference a declared tool. Arguments must parse as a JSON object.
- A named `tool_choice` must reference a declared tool. Any tool choice or
  `parallel_tool_calls` without tools is rejected.
- `auto` maps to `{type:"auto"}`, `required` to `{type:"any"}`, `none` to `{type:"none"}`, and
  a named choice to `{type:"tool",name:"..."}`. A false parallel flag adds
  `disable_parallel_tool_use:true` to `auto`, `any`, and `tool`; `none` already
  disables tool use and accepts no extra member. An absent/true flag adds nothing.

All structural/type/range failures return fixed 400 `invalid_request_body`; recognized but
unsupported semantics return fixed 400 `unsupported_parameter`; an absent provider-required
token limit returns fixed 400 `missing_required_parameter`.

## Response identifier bounds

- Provider message ID: 512 UTF-8 bytes.
- Provider model: 512 UTF-8 bytes; it is validated but the client-visible model is the canonical
  selected model.
- Tool-use ID: 512 UTF-8 bytes.
- Tool name: the same 64-byte ASCII grammar as request tools.
- Total content blocks: 128.
- Incomplete SSE event: 64 KiB.
- Retained non-streaming input: 1 MiB.
- Generated non-streaming output: 1 MiB.
- Generated output for one transport callback: 1 MiB.

## Canonical non-streaming success

`id` comes from the bounded provider message ID. `created` is Unix epoch seconds captured once
when the processor is installed. `model` is the canonical service model, not a provider-returned
alias. Tool-call indexes are consecutive from zero. `content` is null only when the response has
tool calls and no text; otherwise it is the ordered concatenation of text blocks.

```json
{
  "id": "msg_example",
  "object": "chat.completion",
  "created": 1785772800,
  "model": "claude-example",
  "choices": [
    {
      "index": 0,
      "message": { "role": "assistant", "content": "Hello" },
      "finish_reason": "stop"
    }
  ],
  "usage": { "prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18 }
}
```

When tools are present, `message.tool_calls` contains objects of the form
`{"id":"toolu_example","type":"function","function":{"name":"weather","arguments":"{\"city\":\"Paris\"}"}}`.

## Canonical streaming success

Every JSON event is serialized compactly and framed as `data: <json>\n\n`. All chunks use the
same bounded provider ID, captured `created`, canonical model, and
`object:"chat.completion.chunk"`.

1. Initial choice: `{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}`.
2. Text delta: `{"index":0,"delta":{"content":"..."},"finish_reason":null}`.
3. Tool start delta: `delta.tool_calls[0]` contains consecutive `index`, `id`,
   `type:"function"`, function `name`, and empty `arguments`.
4. Tool argument delta: contains only tool `index` and `function.arguments`.
5. Finish chunk: exactly one choice with empty `delta` and one mapped `finish_reason`.
6. Optional usage chunk: `choices:[]` plus normalized OpenAI usage.
7. Sentinel: `data: [DONE]\n\n`.

No `system_fingerprint` or provider-only fields are emitted. A finish reason appears exactly
once. `partial_json` is forwarded as an opaque arguments string; only its enclosing event is
validated.

## Errors

Local request errors use the existing fixed OpenAI envelope. Provider HTTP bodies are never
parsed for client messages and never forwarded.

| Upstream/header condition | Downstream status | Fixed code |
|---|---:|---|
| HTTP 400 | 400 | `provider_invalid_request` |
| HTTP 413 | 413 | `request_too_large` |
| HTTP 429 | 429 | `rate_limit_exceeded` |
| Any other non-200 status | 503 | `service_unavailable` |
| HTTP 200 with wrong media type or content encoding | 503 | `service_unavailable` |
| Known oversized non-stream response | 503 | `response_too_large` |

A failure discovered after an SSE 200 emits exactly one compact fixed event and no `[DONE]`:

```text
data: {"error":{"message":"The AI service stream could not be processed.","type":"api_error","code":"response_translation_failed"}}

```

The processor then records `AiResponseTranslationFailed` once and drains/drops all remaining
upstream bytes. A non-stream failure discovered after status commit emits the corresponding fixed
JSON envelope under that committed status, records the same bounded status, and drains/drops.
