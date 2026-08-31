---
name: streamed-transfer-encoding-preserves-undecoded-codings
description: Use when reviewing how Pingora handles external or application-synthesized non-chunked transfer codings before an HTTP/1 upstream.
status: fixed
finding_id: pingora-review-11
closed: 2026-08-30
---

# Pingora Streamed Transfer Codings — SUPERSEDED BY COMMON INGRESS POLICY

## Current conclusion

The 2026-08-11 accepted tradeoff for client-supplied composite transfer coding
is superseded. The Edgion Pingora fork now applies one common `HttpProxy`
admission rule to every external HTTP/1 request before connection-reuse hooks,
filters, cache lookup, upstream selection, or any request-body pump:

- no `Transfer-Encoding` is accepted; and
- one field whose trimmed value is exactly `chunked`, case-insensitively, is
  accepted.

Every other core-accepted form, including `gzip, chunked`, duplicate `chunked`,
and repeated fields, enters the normal `HTTPStatus(501)` error-rendering path
and is forced non-reusable independently of application renderer and logging
hooks. Generic H1 framing rejections can instead return 400 before
`HttpProxy`; a custom error renderer owns the eventual wire status for the
proxy error. The full current contract, evidence, Envoy comparison, and
upstream revisit trigger live in the Pingora canonical record:

- [Unsupported H1 request transfer coding fails closed](https://github.com/Pandaala/pingora/blob/edgion_v3/edgion-changes/review/http1/h1-unsupported-request-transfer-coding-fail-closed.md)

The re-evaluation trigger requiring a documented common ingress policy has
therefore fired. The old conclusion must not be used to re-admit external
composite transfer coding or to scope validation only to the `Streamed` helper.

## What remains valid from the old rationale

`streamed_transfer_encoding` still preserves non-`chunked` values synthesized
by an application in `upstream_request_filter`. At that point external request
admission has already completed, and those values describe
application-controlled bytes. If the H1 writer has removed a final `chunked`
layer, preserving the remaining coding metadata and appending newly derived
chunk framing keeps the upstream field aligned with those bytes.

This post-admission behavior is not an external-client bypass: the common
admission runs before the application can mutate upstream metadata. Replacing
every application-synthesized value with bare `chunked` would still lie about
bytes that retain another coding, while allowlisting only inside
`streamed_transfer_encoding` would still be the wrong boundary.

## Why the decision changed

The earlier record accepted the client-visible compatibility risk because the
proposed fix was Streamed-only and ordinary routes remained exposed. It named
a common ingress rule covering Ordinary and Streamed paths as the correct
re-evaluation trigger. The current fork implements exactly that boundary and
adds out-of-band origin evidence for both H1 and H2 upstreams, legal plain
`chunked` controls, reuse-hook ordering coverage, and a sequential same-socket
custom renderer/logger regression.

The inherited limitation is that Pingora removes downstream chunk framing but
does not decode preceding transfer codings, while standard hop-by-hop
sanitization removes the external `Transfer-Encoding` field before forwarding.
Forwarding those still-coded bytes would change their declared semantics. The
fork therefore fails closed instead of carrying a transfer-coding decoder or
an incomplete coding registry.

## Current non-accepted alternatives

- Restore external composite transfer-coding forwarding based on the old
  accepted-tradeoff conclusion.
- Validate only `UpstreamRequestBodyDisposition::Streamed`; ordinary and H2
  upstream conversion must share the same external admission boundary.
- Replace residual application-synthesized coding metadata with bare
  `chunked`; that can make the field disagree with application-owned bytes.
- Vendor a transfer-coding decoder or preserve client hop-by-hop metadata as a
  speculative local workaround.

## Re-evaluation triggers

Revisit the common admission only when upstream Pingora adopts and tests an
equivalent pre-routing terminal rejection, or decodes every removed coding and
accurately reframes both H1 and H2 upstream bodies. Preserve the negative
origin evidence and legal plain-`chunked` controls when evaluating that change.

## Reference cases

- `pingora-review-11`, accepted on 2026-08-11 and superseded on 2026-08-30.
- Pingora `proxy_common.rs::h1_transfer_encoding_is_forwardable` and
  `lib.rs::process_new_http` own external request admission.
- Pingora `proxy_h1.rs::{apply_upstream_body_disposition,
  streamed_transfer_encoding}` owns post-admission application-synthesized
  metadata.
- Edgion `edgion-gateway/src/routes/http/proxy_http/mod.rs::upstream_request_body_disposition`.
- RFC 9112 Sections 6.1 and 7; IANA HTTP Transfer Coding Registry.
