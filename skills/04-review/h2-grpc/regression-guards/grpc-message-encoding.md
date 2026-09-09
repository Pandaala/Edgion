---
name: grpc-message-encoding
description: Shared gRPC terminal writer percent-encodes messages, preserving spaces and escaping percent, controls, and non-ASCII UTF-8 bytes.
knowledge_type: regression-guards
---

# h2-grpc-02 — Shared `grpc-message` percent-encoding

**Status:** ✅ fixed · 2026-05-18
**Spec:** gRPC over HTTP/2 spec — "grpc-message MUST be percent-encoded as described in RFC 3986 §2.1"

## Conclusion

`send_grpc_trailers_only_frame` routes canonical messages through
`percent_encode_grpc_message` before header-value parsing. Native gRPC carries them
in its single HEADERS+END_STREAM reply; gRPC-Web carries them in the trailer map
encoded by the bridge. HTTP-mapped and exact-status writers, including
`PingoraSessionAdapter`, reuse this terminal writer.

The former `send_grpc_error_three_frame` name describes historical implementation,
not a current caller. This encoding guard does not establish that arbitrary queued
response headers have correct ownership or that every client interprets conflicts alike.

## Spec clause

> "grpc-message: arbitrary string, but its value MUST be percent-encoded as described in
> RFC 3986 §2.1 before being placed in the header."
> — gRPC over HTTP/2 spec

## Code shape today

- `edgion-gateway/src/runtime/server/error_response.rs::percent_encode_grpc_message` —
  `pub(crate)` helper: `utf8_percent_encode(s, GRPC_MESSAGE_ASCII_SET).to_string()`.
  `GRPC_MESSAGE_ASCII_SET` is `CONTROLS.add(b'%')`: space stays unchanged;
  percent, controls and non-ASCII UTF-8 bytes are encoded.
- `send_grpc_trailers_only_frame` encodes each supplied canonical message before
  parsing the resulting ASCII header value. The adapter delegates here rather
  than maintaining an independent encoder.
- Unit tests: `percent_encode_grpc_message_ascii_passthrough`, `_non_ascii_encoded`,
  `_control_chars_encoded`, `_percent_encoded` — all in the `tests` module of
  `error_response.rs`. The accented Latin and Greek fixtures cover two-byte UTF-8;
  they do not establish three-byte coverage. Test names are verification targets,
  not a claim of a fresh run.

## Encoding set rationale

`CONTROLS` encodes bytes 0x00-0x1F and 0x7F. Adding `%` excludes the escape
introducer, leaving exactly 0x20-0x24 and 0x26-0x7E unencoded. The library always
encodes non-ASCII UTF-8 bytes. Thus `quota 50% exceeded` becomes
`quota 50%25 exceeded`, and literal `%41` becomes `%2541` rather than an ambiguous
pre-encoded value.

## Re-evaluation triggers

- `http_to_grpc_status` is extended with non-ASCII messages — the encoding will fire and
  must be verified in tests.
- The shared encoding set changes, an adapter stops using the shared terminal writer,
  or a new path inserts a canonical message without encoding — revalidate its actual
  output and callers instead of relying on the historical fixed status.

## Reference

- Historical verifier (source queue removed): `tasks/todo/rfc-review/_raw/verification-a2.md → V2` (CONFIRM)

## Resolution references

- `h2-grpc-02` — 2026-05-18.
