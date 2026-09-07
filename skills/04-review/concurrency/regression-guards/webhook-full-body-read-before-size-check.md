---
name: webhook-full-body-read-before-size-check
description: Read when reviewing LinkSys webhook `webhook_call` response-body size-limit findings — full body read via `resp.bytes()` before the size check, `maxResponseBytes` enforced only after buffering, no `Content-Length` pre-check, `read_body_limited` truncating only after buffering (link-sys-F019).
knowledge_type: regression-guards
---

# LinkSys webhook `webhook_call` full response-body read before size check (link-sys-F019)

**Status / verdict**: fixed (link-sys-F019, 2026-05-25)
**Note (2026-07-08)**: the legacy webhook `call.rs` module this describes was removed
entirely as dead code (production had moved to the shared `stream_body_capped` in
`plugins/http/common/outbound/call.rs`). Kept for historical design-rationale context.

When reviewing LinkSys webhook findings of the form "`webhook_call` reads the full response body via `resp.bytes()` before the size check / `maxResponseBytes` is enforced only after buffering the whole body / no `Content-Length` pre-check / `read_body_limited` truncates only after buffering (link-sys-F019)": **fixed (link-sys-F019, 2026-05-25)**. `webhook_call` (formerly `edgion-gateway`'s `link_sys/providers/webhook/call.rs`) now reads through `stream_body_capped` — a `bytes_stream()` accumulator capped at `max_bytes` (peak memory ≤ cap + one chunk) — plus a success-path `Content-Length` early reject, mirroring the established `core/common/http_util::fetch_json_limited` (OIDC) and `all_endpoint_status::read_body_limited` patterns. Equality semantics are preserved (`len == max_bytes` is not too-large), and the non-success path still truncates the diagnostic body (no Content-Length short-circuit there). Re-reports of the "full body buffered before size check / no Content-Length pre-check" form are not accepted; verify the current `webhook_call` calls `stream_body_capped` before re-reporting.

## Historical resolution evidence: link-sys-F019

`webhook_call` (formerly `edgion-gateway`'s `link_sys/providers/webhook/call.rs`, module removed 2026-07-08 as dead code) buffered the **entire** response body via `resp.bytes().await` before enforcing `max_bytes` (= `min(config.max_response_bytes, global webhook_max_response_bytes)`, defaults 1 KB / 16 KB): the success path checked `b.len() > max_bytes` only *after* the full allocation, and the non-success `read_body_limited` truncated only *after* buffering the whole body. A misconfigured / compromised operator webhook streaming a multi-MB/GB body was fully allocated before `ResponseTooLarge` fired (transient memory pressure under concurrency; bounded by the operator trust boundary; reachable on the data-plane via `webhook::resolver`). No `Content-Length` pre-check existed. Fixed: a shared `stream_body_capped(resp, max_bytes) -> Result<(Bytes, bool /*truncated*/), reqwest::Error>` reads via `bytes_stream()` and stops the moment the running total would exceed the cap (peak memory ≤ cap + one chunk); the success path adds a `resp.content_length() > max_bytes` early reject (zero body read for honest oversized servers) then maps `truncated → ResponseTooLarge`, and `read_body_limited` delegates to the same helper for the truncated diagnostic body. Mirrors the established `core/common/http_util::fetch_json_limited` (OIDC) and `all_endpoint_status::read_body_limited` patterns. Equality semantics preserved (`len == max_bytes` is not too-large); non-success diagnostic body still truncates (no Content-Length short-circuit there, to keep the prior diagnostic behavior). Unit regressions `chunked_response_exceeding_cap_returns_too_large` (dishonest chunked, no `Content-Length` → exercises the streaming guard, not the CL pre-check) + `chunked_response_within_cap_returns_full_body`. Re-reports of the "reads full body before size check / no Content-Length pre-check" form are not accepted; verify `webhook_call` calls `stream_body_capped` before re-reporting.

## Resolution references

- `link-sys-F019` — 2026-05-25.
