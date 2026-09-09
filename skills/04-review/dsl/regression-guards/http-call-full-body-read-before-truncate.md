---
name: http-call-full-body-read-before-truncate
description: Read when reviewing DSL `http.call` response aggregation. The current guard streams through the canonical capped helper and fails the call on overflow; historical read-all and silent-truncation behavior is superseded.
knowledge_type: regression-guards
---

# DSL `http.call` response aggregation is bounded while streaming

**Status / verdict**: fixed and strengthened.

`http.call` is a plugin-initiated outbound exchange, so it may aggregate a response only behind a hard byte cap. `complete_http_call_response` computes `min(httpMaxResponseBodyBytes, max_string_len)`, rejects a declared `Content-Length` above it, and passes the response to the canonical `common::outbound::call::stream_body_capped` helper. A stream error or the helper's `truncated` result discards the complete result and returns `{status: 0, body: "", headers: {}}`.

The helper must inspect end-of-stream after accumulating exactly the cap. Therefore an exact-cap body succeeds only after EOF, while one additional byte fails without reading an unbounded tail. A zero cap similarly accepts an empty body only after EOF and rejects the first nonempty byte.

After bounded collection, valid UTF-8 is reused directly. Invalid UTF-8 first receives an exact output-length preflight with checked arithmetic; replacement conversion is allocated only if its output fits `max_string_len`. Never convert lossy text and then measure or trim it.

## Historical resolution evidence

Before plugins-http-dsl-F007 (2026-05-26), `call_builtin_http` used `resp.bytes().await` and applied its configured cap only after the full response was allocated. F007 changed this to streaming accumulation but preserved silent prefix truncation. Both the original full-body read and that intermediate truncation contract are now superseded: current root architecture requires overflow to fail and forbids partial response data.

Regression tests in `tests/http_call_tests.rs` use synthetic streams, including panic-on-poll tails, so the bounds are verified without network access. Do not replace the canonical helper with a local stream loop, add a response-side `BodyStorage`, or apply this bounded outbound-call behavior to the main proxy upstream, which remains streaming.
