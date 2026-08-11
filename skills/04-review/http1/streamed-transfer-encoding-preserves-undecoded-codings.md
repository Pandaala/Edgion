---
name: streamed-transfer-encoding-preserves-undecoded-codings
description: Use when reviewing claims that Pingora Streamed requests should allowlist or discard client-supplied non-chunked transfer codings before an HTTP/1 upstream.
status: accepted-tradeoff
finding_id: pingora-review-11
closed: 2026-08-11
---

# Pingora Streamed Transfer Codings — ACCEPTED-TRADEOFF

## Conclusion

For a valid sequence containing one final `chunked` coding, Pingora's `Streamed` HTTP/1 path
intentionally preserves transfer codings it did not decode and replaces that decoded final layer.
Edgion accepts the residual risk from an upstream that misparses an otherwise correctly framed
`Transfer-Encoding` value.

Fix suggestions of the form "allowlist transfer codings only in
`streamed_transfer_encoding`" are **not accepted**.

## Core Rationale

**1. The header must continue to describe the forwarded bytes**

For the valid single-final-`chunked` sequence covered by this decision, Pingora decodes the
downstream chunk framing but does not decode earlier transfer codings. RFC 9112 Section 6.1 permits
an intermediary to decode or add transfer codings only when it makes the corresponding field-value
changes. Preserving non-`chunked` codings and appending a newly derived final `chunked` therefore
keeps the upstream message accurate; dropping a coding such as `gzip` while forwarding the
still-coded bytes would corrupt the message semantics.

The downstream H1 validator separately requires the final coding to be `chunked`, and the upstream
writer emits its own valid chunk framing. The validator does not currently reject another
`chunked` earlier in the sequence; that distinct metadata-rewrite defect is tracked separately as
Pingora finding 33 and is not accepted by this decision.

**2. A Streamed-only allowlist is not an Edgion ingress policy**

Edgion selects `UpstreamRequestBodyDisposition::Streamed` only after a request plugin installs a
per-chunk body handler, currently ExtProc `STREAMED`. Ordinary routes retain the TE field and its
non-`chunked` coding sequence while the transport parses and regenerates chunk framing. They can
therefore forward the same non-`chunked` transfer codings. Restricting only the Streamed helper
would make acceptance depend on plugin attachment without removing the alleged exposure from
ordinary routes.

If Edgion later needs to protect a specifically identified non-conforming upstream, the policy
belongs at a common ingress validation boundary and must cover both Ordinary and Streamed paths.
That is a separate client-visible compatibility decision.

**3. The proposed coding list is not the transfer-coding registry**

The proposed list mixed content codings with transfer codings: `br` and `zstd` are registered HTTP
content codings, while the HTTP Transfer Coding Registry includes deprecated aliases such as
`x-gzip` and `x-compress`. A hard-coded partial list would reject valid extension traffic while
providing inconsistent protection.

RFC 9112 says that a server receiving an unsupported request transfer coding should respond with
501. The finding requires an upstream that violates that parsing behavior and turns the value into
a usable message-boundary disagreement; no such Edgion backend or reproducible desynchronization
has been identified.

## Fix Suggestions Not Accepted

- Allowlist codings inside `streamed_transfer_encoding` — it does not cover the Ordinary path and
  turns request-body plugin attachment into an undocumented admission policy.
- Replace every Streamed value with bare `chunked` — this lies about bytes that retain an earlier
  transfer coding.
- Treat the hypothetical behavior of any lenient upstream as a demonstrated Pingora desync for a
  valid single-final-`chunked` sequence — the current writer emits unambiguous final chunk framing,
  and exploitability requires a concrete divergent upstream parser.

## Re-evaluation Triggers

Re-open this decision only if:

- A supported Edgion upstream is shown to parse a forwarded transfer-coding sequence divergently,
  with a reproducible request-boundary desynchronization.
- Edgion adopts a documented, common ingress policy that rejects unsupported transfer codings on
  every request path, with an explicit compatibility decision.
- Pingora starts decoding or mutating a non-`chunked` transfer coding without updating the
  `Transfer-Encoding` field accordingly.

## Reference Cases

- `pingora-review-11`, reviewed and accepted on 2026-08-11.
- Pingora finding 33 tracks the separate repeated/non-final `chunked` rewrite defect.
- Pingora `proxy_h1.rs::{apply_upstream_body_disposition, streamed_transfer_encoding}`.
- Edgion `edgion-gateway/src/routes/http/proxy_http/mod.rs::upstream_request_body_disposition`.
- RFC 9112 Sections 6.1 and 7; IANA HTTP Transfer Coding Registry.
