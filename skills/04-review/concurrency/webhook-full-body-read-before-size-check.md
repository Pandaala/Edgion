---
name: webhook-full-body-read-before-size-check
description: Read when reviewing LinkSys webhook `webhook_call` response-body size-limit findings — full body read via `resp.bytes()` before the size check, `maxResponseBytes` enforced only after buffering, no `Content-Length` pre-check, `read_body_limited` truncating only after buffering (link-sys-F019).
---

# LinkSys webhook `webhook_call` full response-body read before size check (link-sys-F019)

**Status / verdict**: fixed (link-sys-F019, 2026-05-25)

When reviewing LinkSys webhook findings of the form "`webhook_call` reads the full response body via `resp.bytes()` before the size check / `maxResponseBytes` is enforced only after buffering the whole body / no `Content-Length` pre-check / `read_body_limited` truncates only after buffering (link-sys-F019)": **fixed (link-sys-F019, 2026-05-25)**. `webhook_call` (`src/core/gateway/link_sys/providers/webhook/call.rs`) now reads through `stream_body_capped` — a `bytes_stream()` accumulator capped at `max_bytes` (peak memory ≤ cap + one chunk) — plus a success-path `Content-Length` early reject, mirroring the established `core/common/http_util::fetch_json_limited` (OIDC) and `all_endpoint_status::read_body_limited` patterns. Equality semantics are preserved (`len == max_bytes` is not too-large), and the non-success path still truncates the diagnostic body (no Content-Length short-circuit there). Re-reports of the "full body buffered before size check / no Content-Length pre-check" form are not accepted; verify the current `webhook_call` calls `stream_body_capped` before re-reporting.
