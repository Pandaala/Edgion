# Clarify Redirect Connection Policy

## Status

Pending design decision; discovered during B0194 redirect log redaction.

## Evidence

`edgion-gateway/src/routes/http/redirect_http.rs` inserts `Connection: close`
without disabling session keepalive. Pingora revision `518f727` H1
`respect_keepalive` defaults ordinary HTTP/1.1 requests to keepalive, and
`prepare_response_header_for_write` overwrites Connection from `will_keepalive`.
The same behavior exists before B0194.

## Required Follow-up

Decide whether the dedicated redirect service should force closure or retain
normal HTTP keepalive. Then align the handler's session policy and header with
that decision, and verify client-close and ordinary HTTP/1.1 requests using
in-memory sessions. Existing campaign integration files remain frozen.
