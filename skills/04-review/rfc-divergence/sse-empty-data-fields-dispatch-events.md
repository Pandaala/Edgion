---
name: sse-empty-data-fields-dispatch-events
description: Use when reviewing whether an empty SSE data field should dispatch, or Anthropic-compatible heartbeat handling.
---

# Empty SSE data fields dispatch events

The WHATWG EventSource algorithm appends the field value and then one LF for every `data` field.
Consequently, a block containing one empty `data:` or bare `data` field has a non-empty internal
data buffer and dispatches an event whose data becomes the empty string after the trailing LF is
removed. Two empty data fields dispatch one event containing a single newline. A block with no data
field does not dispatch.

`SseDecoder` represents the unmaterialized trailing LF with `cur_has_data` and stores separators
only between multiple fields. Do not replace that predicate with `!cur_data.is_empty()`; doing so
would incorrectly drop a standard empty-data event.

Anthropic response translation remains stricter than generic SSE framing. Anthropic Messages events
carry named JSON payloads, including ping events. An empty-data event cannot be parsed as that
provider protocol and is therefore a translation failure, even though the shared decoder correctly
recognizes it as an SSE event.

References:

- [WHATWG HTML, Server-sent events](https://html.spec.whatwg.org/multipage/server-sent-events.html)
- [Anthropic streaming Messages](https://platform.claude.com/docs/en/build-with-claude/streaming)
