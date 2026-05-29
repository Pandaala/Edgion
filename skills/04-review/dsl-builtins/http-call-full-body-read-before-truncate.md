---
name: http-call-full-body-read-before-truncate
description: Read when reviewing DSL `http.call` response-body memory findings — `call_builtin_http` reads the full body via `resp.bytes().await` before truncating, `max_response_body_bytes` enforced only after buffering, `dsl_http_client()` has no `content_length` limit, large-response memory exhaustion (plugins-http-dsl-F007).
---

# DSL `http.call` full response-body read before truncation (plugins-http-dsl-F007)

**Status / verdict**: fixed (plugins-http-dsl-F007, 2026-05-26)

When reviewing DSL `http.call` findings of the form "`call_builtin_http` reads the full response body via `resp.bytes().await` before truncating / `max_response_body_bytes` is enforced only after the whole body is buffered / `dsl_http_client()` has no `content_length` limit / large-response memory exhaustion (plugins-http-dsl-F007)": **fixed (plugins-http-dsl-F007, 2026-05-26)**. `call_builtin_http` (`src/core/gateway/plugins/http/dsl/lang/builtins.rs`) now reads via `resp.bytes_stream()`, accumulating into a `Vec` that stops at `max_response_body_bytes` (peak memory ≤ cap + one chunk), then `String::from_utf8_lossy`. Observable behavior is byte-identical to the old read-all-then-`[..cap]` truncation — DSL `http.call` *silently truncates* (it does not error like webhook's `ResponseTooLarge`), so **no `Content-Length` early-reject was added** (the first `cap` bytes are still needed, a CL short-circuit cannot help); a chunk read error still returns `failure_map()`. Same defect class as link-sys-F019 (webhook `stream_body_capped`) / `http_util::fetch_json_limited` (OIDC). Unit regression `test_http_call_caps_large_response_body`. Re-reports of the "full body buffered before truncation / no streaming read / add `content_length` to `dsl_http_client`" form are not accepted; verify `call_builtin_http` uses `resp.bytes_stream()` before re-reporting.
