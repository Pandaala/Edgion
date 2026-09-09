# Clarify Redirect Connection Policy

## Status

Pending design decision; independent of redirect log redaction.

## Evidence

`edgion-gateway/src/routes/http/redirect_http.rs` inserts `Connection: close`
without disabling session keepalive. Pingora revision `518f727` H1
`respect_keepalive` defaults ordinary HTTP/1.1 requests to keepalive, and
`prepare_response_header_for_write` overwrites Connection from `will_keepalive`.
This is a connection-policy question, independent of log redaction.

## Required Follow-up

Decide whether the dedicated redirect service should force closure or retain
normal HTTP keepalive. Then align the handler's session policy and header with
that decision, and verify client-close and ordinary HTTP/1.1 requests using
in-memory sessions. Existing integration files require explicit authorization before modification.
