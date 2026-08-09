# gw-16 — A bodyless upstream response never reaches `on_chunk`, so the AI translator emits no error envelope and no body at all

| Field | Value |
|-------|-------|
| ID | `gw-16` |
| Disposition | `confirmed` |
| Severity | P2 |
| Dimension | Correctness / client contract |
| Location | `edgion-gateway/src/ai/anthropic/response.rs:202-236`, `:275-285`, `:642-659`; `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_body_filter.rs:46-69`, `:77-89` |
| Reviewer | Streaming / translation review |
| Status | open |

## Summary

`AnthropicResponseProcessor` can only write its fixed OpenAI error envelope from `on_chunk`, but Pingora never runs `upstream_response_body_filter` when the upstream response header task already carries `end_of_stream = true`. A bodyless provider response therefore reaches the client as a status-only reply with `Content-Type: application/json` and an empty body.

## Details

`normalize_header` only mutates the header and stores `self.mode = BodyMode::Fixed(error)`; the envelope bytes are produced later in `process_fixed` (`*chunk = Some(Bytes::from(render_fixed_error(error)))`).

`run_stream_processors` is invoked only from `upstream_response_body_filter`, and in pingora's `upstream_filter` only `HttpTask::Body` dispatches there — `HttpTask::Header(h, true)` and `HttpTask::Done` do not. So for an upstream reply with `Content-Length: 0`, or any bodyless 4xx/5xx from an edge/LB in front of the provider, `on_chunk` is never called.

`strip_provider_headers` has already removed `content-length`/`transfer-encoding`, and `h1_response_filter` skips its chunked upgrade when `end == true`, so `init_body_writer_comm` falls to `init_close_delimited()` and pingora additionally does `set_keepalive(None)`.

The same gap swallows `BodyMode::AwaitingHeader` and `RESPONSE_TOO_LARGE`, and `record_translation_failure` (`EdgionStatus::AiResponseTranslationFailed`) is never recorded for those cases.

## Impact

A bodyless `502`/`503` from the provider's edge is forwarded as `503` + `Content-Type: application/json` + zero bytes. An OpenAI SDK calls `response.json()` and raises a JSON decode error instead of surfacing a structured `service_unavailable`; the operator gets no `AiResponseTranslationFailed` signal; the downstream keep-alive connection is torn down.

Worst shape: a bodyless upstream `200` becomes a silent, well-formed-looking `200` with an empty body and no error status at all.

This also contradicts `tasks/archive/completed-tasks/ai-gateway-anthropic-translation/04-wire-contract.md:136-144`, which promises a fixed envelope for every listed upstream condition.

## Suggested fix

Give the pipeline an end-of-response entry point that runs even when no body task arrives — either have `upstream_response_filter` (or a `response_done` hook) call `run_stream_processors(session, ctx, &mut None, true)` when the final header task carries `end_of_stream`, or make `on_response_header` able to return a replacement body that `pg_upstream_response_filter` writes as a local reply.

## Notes

The most plausible trigger is an edge/LB in front of the provider rather than the provider itself; this was not reproduced end-to-end, but the code path is unconditional and does not depend on which upstream produces the bodyless response.
