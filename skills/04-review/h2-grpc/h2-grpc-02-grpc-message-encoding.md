---
name: h2-grpc-02-grpc-message-encoding
description: Fixed — grpc-message in send_grpc_trailers_only now percent-encodes per gRPC spec (RFC 3986 §2.1). Reject re-reports claiming the encoding is missing.
---

# h2-grpc-02 — `grpc-message` percent-encoding in `send_grpc_trailers_only`

**Status:** ✅ fixed · 2026-05-18
**Spec:** gRPC over HTTP/2 spec — "grpc-message MUST be percent-encoded as described in RFC 3986 §2.1"

## Conclusion

`send_grpc_trailers_only` now routes `grpc-message` through `percent_encode_grpc_message`
before inserting it into the trailer map. All current static messages ("invalid argument",
"not found", etc.) are pure ASCII, so the wire output is unchanged. Future non-ASCII
messages will be correctly encoded rather than causing a `HeaderValue::from_static` panic
or landing on the wire unencoded.

Suggestions of the form "send_grpc_trailers_only uses from_static and bypasses encoding"
or "grpc-message is not percent-encoded" are **not accepted** — the fix is in place.

## Spec clause

> "grpc-message: arbitrary string, but its value MUST be percent-encoded as described in
> RFC 3986 §2.1 before being placed in the header."
> — gRPC over HTTP/2 spec

## Code shape today

- `src/core/gateway/runtime/server/error_response.rs::percent_encode_grpc_message` —
  `pub(crate)` helper: `utf8_percent_encode(s, CONTROLS).to_string()`. Keeps ASCII
  visible chars (0x21-0x7E) and space (0x20); percent-encodes control chars and non-ASCII
  UTF-8 bytes. Matches the encoding used by `session_adapter::send_grpc_error_direct`.
- `send_grpc_trailers_only` calls `percent_encode_grpc_message(grpc_message)` and wraps
  the result in `HeaderValue::from_str(...)` (infallible for the resulting ASCII string).
- Unit tests: `percent_encode_grpc_message_ascii_passthrough`, `_non_ascii_encoded`,
  `_control_chars_encoded` — all in the `tests` module of `error_response.rs`.

## Encoding set rationale

`CONTROLS` (`percent_encoding` crate) encodes bytes 0x00-0x1F and 0x7F, leaving
0x20-0x7E (space + all ASCII printable) as-is. This is equivalent to session_adapter.rs's
hand-rolled `is_ascii_graphic() || c == ' '` check, ensuring both gRPC error paths
produce identical output for the same message string.

## Re-evaluation triggers

- `http_to_grpc_status` is extended with non-ASCII messages — the encoding will fire and
  must be verified in tests.
- The encoding set used by `session_adapter::send_grpc_error_direct` changes — update
  `percent_encode_grpc_message` accordingly to keep the paths consistent.

## Reference

- Verifier: `tasks/todo/rfc-review/_raw/verification-a2.md → V2` (CONFIRM)
- Closure: [CLOSED-FINDINGS.md](../CLOSED-FINDINGS.md) → h2-grpc-02
