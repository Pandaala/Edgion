---
name: sse-empty-data-fields-dispatch-events
description: Use when reviewing whether an empty SSE data field should dispatch, or Anthropic-compatible heartbeat handling.
knowledge_type: review-evidence
---

# Empty SSE data fields dispatch events

The WHATWG EventSource algorithm appends the field value and then one LF for every `data` field.
Consequently, a block containing one empty `data:` or bare `data` field has a non-empty internal
data buffer and dispatches an event whose data becomes the empty string after the trailing LF is
removed. Two empty data fields dispatch one event containing a single newline. A block with no data
field does not dispatch.

`SseDecoder` preserves one raw data range per field, including a zero-length range for an
empty field. Completed blocks classify as Data when `parsed.data` has a range, and Dataless
when no data field exists; `semantic_data` inserts separators only between fields.
Do not infer classification from semantic payload length: that would drop a standard
empty-data event. Canonical Chat observation preserves this typed distinction; see
[the regression contract](../regression-guards/canonical-chat-block-classification.md).

Anthropic response translation remains stricter than generic SSE framing. Anthropic Messages events
carry named JSON payloads, including ping events. An empty-data event cannot be parsed as that
provider protocol and is therefore a translation failure, even though the shared decoder correctly
recognizes it as an SSE event.

References:

- [WHATWG HTML, Server-sent events](https://html.spec.whatwg.org/multipage/server-sent-events.html)
- [Anthropic streaming Messages](https://platform.claude.com/docs/en/build-with-claude/streaming)

## Resolution references

- `gw-20` — 2026-08-09 (rejected false positive).
