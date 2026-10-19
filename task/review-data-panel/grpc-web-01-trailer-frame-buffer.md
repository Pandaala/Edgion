# Evaluate a single-buffer gRPC-Web trailer frame

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Edgion baseline: `665963fae`.
Inspected Pingora fork: `1aaa45cc70430bc782990bc20ddee3fef36d6b99`.
Inspected bytes version: 1.12.1 from the workspace lockfile.

## Evidence

The fork's protocols/http/bridge/grpc_web.rs::response_trailer_filter requests
BytesMut capacity 256 and split_off(5) for its trailer payload. It writes every
HeaderMap entry as raw name, colon, value and CRLF. After checked conversion of
payload length to u32, the prefix receives 0x80 and the big-endian length. unsplit
recombines the views, freeze yields owned Bytes, and the bridge enters Done.

bytes::BytesMut::split_off shallow-clones its ownership metadata. A Vec-backed
buffer is promoted to a separately allocated Shared owner. try_unsplit combines
adjacent views of that same owner without copying payload. If growth separates
the views, unsplit instead extends the prefix with the payload bytes. It is
incorrect to describe every current trailer as copying through two buffers.

A single BytesMut with five placeholder bytes could append payload directly,
validate its length, then fill the prefix. This could avoid split ownership and
some growth-related copy work. It does not establish a net allocation saving:
freeze/Vec-to-Bytes conversion can itself allocate ownership metadata depending
on final length/capacity. Retained capacity, growth copies and peak overlap must
be measured across both paths. The final owned frame is required by asynchronous
downstream delivery; borrowing the HeaderMap is not a substitute.

## Preserved boundaries

Keep HeaderMap traversal order, repeated entries, raw header bytes, colon/CRLF
encoding, the five-byte prefix and checked u32 payload length. Preserve state on
length failure: Done is assigned only after successful construction. A direct
bridge call outside Trailers changes state to Disabled and returns None. The
outer module separately returns None without invoking the bridge when trailers
are absent; these are different cases.

Do not mix request/response Content-Type normalization changes into this buffer
proposal. The current request path lowercases and globally replaces its media
prefix; response replacement is also global. Prefix-only rewriting would require
separate semantic evidence. Preserve raw protocol activation, WebSocket precedence,
HTTP/1-only gRPC-Web availability and ordinary/native-gRPC disabled behavior.

The existing outer module storage proposal in context-01 remains separate. This
candidate belongs in the external Pingora fork and needs explicitly scoped
implementation/review and dependency-update authority. Do not edit Cargo's checkout
or silently advance this workspace's pinned revision to implement it.

## Acceptance

Compare exact frame bytes and parsed lengths for absent/empty trailers, repeated
keys, non-text values, small frames, payloads around the initial capacity boundary,
and larger growth cases. Cover state repetition and checked-length failure using
an appropriate isolated seam without allocating a multi-gigabyte fixture.
Measure allocation calls/requested bytes, growth/copy behavior, final capacity and
peak live storage including freeze. Keep fixture setup outside measured sections.

Exercise actual H1 gRPC-Web trailer delivery and the existing module activation
matrix after any authorized implementation. Unit output equality alone does not
prove cancellation, compression/framing interplay or live delivery. Preserve
streaming and request-only body storage; introduce no metrics, per-chunk logs or
new buffering policy. No implementation or fresh allocation probe was made here.

Original authority: skills/04-review/h2-grpc/regression-guards/grpc-web-independent-of-frontend-http2.md
and skills/04-review/http1/compatibility-constraints/compression-requires-known-response-length.md.
