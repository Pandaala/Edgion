# gw-ai3-04: Native Messages metadata inspection rejects valid long conversations (>4096 messages or content blocks) as "malformed body"

- **Category**: design-error
- **Severity**: P2
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/ai/protocols/anthropic/request.rs:79-115` (`each`: `count > 4096` → custom error → `MalformedBody`)
  - `edgion-gateway/src/ai/protocols/anthropic/request.rs:119-156` (`inspect_block`: `remaining == 0` → `UnsupportedParameter`, mapped to `MalformedBody` by `each`)
  - `edgion-gateway/src/ai/protocols/anthropic/request.rs:157-224` (`metadata`: `remaining = 4096` across all messages and nested `tool_result` blocks)
  - `edgion-gateway/src/ai/http/proxy.rs:928-944` (`canonical_messages_request` calls `metadata` for every native Messages request)

## Summary

To decide whether a native Messages request is "binding" (thinking blocks, file/container
references → no retry, no model change) or "composite" (advisor tools, fallbacks, compaction →
usage observation unavailable), AiProxy walks the request with a borrowed visitor. The walk is
capped at 4096 elements per array and 4096 content blocks in total, and exceeding the cap returns
`AiProxyError::MalformedBody` (400 `invalid_request_body`, "The request body must be a valid JSON
object."). Long agentic conversations — exactly the traffic native Messages is used for — routinely
contain thousands of `tool_use` / `tool_result` blocks; Anthropic's own limit is 100,000 messages
per request. Such requests are refused by the gateway with a misleading error even though they are
valid and within the configured body cap.

## Current behavior (evidence)

- `each()` (`request.rs:96-106`): every `messages`, `content`, `tools`, `edits` and nested
  `tool_result.content` array aborts after 4096 elements with
  `serde::de::Error::custom("native metadata inspection limit")`, which `each` maps to
  `AiProxyError::MalformedBody` (`request.rs:113`).
- `inspect_block()` (`request.rs:125-128`): a shared `remaining = 4096` budget across *all*
  messages' content blocks (including nested tool results) returns `UnsupportedParameter`, which
  the enclosing `each` again converts to `MalformedBody`.
- The JSON body has already been fully parsed by `serde_json::from_str` into borrowed `RawValue`s
  (`fields()`), so the cap is not protecting memory; inspection is O(n) over a body already bounded
  by `requestBody.maxBodySize`.
- OpenAI Chat and Responses requests have no equivalent walk and no such limit, so the same
  conversation is accepted on `/v1/chat/completions` and rejected on `/v1/messages`.

Example: a coding-agent session with 1,800 turns, each assistant turn holding text + `tool_use`
and each user turn a `tool_result`, has ~5,400 content blocks → 400 from the gateway.

## Industry comparison

Envoy AI Gateway, LiteLLM and Kong AI Proxy forward native Anthropic requests without a
structural element cap below the provider's own limits; request size is bounded by byte limits
(Envoy `per_connection_buffer_limit_bytes`, NGINX `client_max_body_size`), not by element counts.

## Impact

Hard, non-retryable failures for the longest (and most valuable) agentic sessions, reported to the
client as a malformed body, which SDKs and agents do not retry or explain.

## Suggested direction

- When the inspection budget is exhausted, classify conservatively instead of rejecting: set
  `binding = true` (no retry / model change) and `composite = true` (observation unavailable,
  which already routes to the existing composite rules at `proxy.rs:450-460`). This preserves every
  safety property the walk exists for.
- If a CPU bound is still wanted, derive it from the body byte cap (work units per byte), not from a
  fixed element count, and report `RequestTooLarge` (413) rather than `MalformedBody`.

## Notes

- No `skills/04-review` page documents the 4096 element limit or its client-visible behavior
  (searched ai-routing, ai-quota, http-body). `native-messages-preservation-and-accounting.md`
  covers the binding/composite classification but not the rejection on overflow.
- Not tracked under `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P2 unchanged.
- `ai/protocols/anthropic/request.rs:96-106`: `each()` errors when `count > 4096` and maps the error to `AiProxyError::MalformedBody` (`:113`).
- `inspect_block` (`:119-128`) returns `UnsupportedParameter` when the shared `remaining` budget (`let mut remaining = 4096`, `:209`) is spent. This covers all array-form content blocks, including nested `tool_result`. The enclosing `each` converts it to `MalformedBody`.
- The body is already parsed into borrowed `RawValue`s, so the cap bounds CPU only and protects no memory. No review page documents the limit. Note: string-form `content` messages do not consume the budget, so only block-form agent transcripts hit it. With 200k–1M-token context windows that is reachable.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P2 unchanged.
- `ai/protocols/anthropic/request.rs` diff only drops `Default` from `Item`; `each()` still errors at `count > 4096` (`request.rs:100-101`) and maps to `MalformedBody` (`:113`).
- The shared `let mut remaining = 4096;` budget (`request.rs:211`) and `inspect_block` (`:119`) are unchanged; `http/proxy.rs:928-929` still calls `metadata` for every native Messages request.
