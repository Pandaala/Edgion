# Response compression requires a known response length

Pingora's gzip response encoder writes each upstream body chunk into a
stateful deflate stream but finishes the encoder only at end-of-stream. It does
not expose a per-chunk flush through `ResponseCompression`. A low-rate stream
can therefore produce no downstream bytes until the encoder fills its internal
buffer or the response ends.

When compression is configured, Edgion initializes Pingora's module at the
default level so its request filter captures the client's `Accept-Encoding`.
It then makes the final decision in `ProxyHttp::response_filter`. This is the
last Edgion header hook before Pingora's downstream modules and is shared by
origin and cache-hit responses. Gzip is enabled only when all of these
conditions hold:

- `server.enableCompression` is `true`;
- the final client-visible response has exactly one valid `Content-Length`;
- the response has no `Transfer-Encoding`; and
- the response is not classified as SSE.

An unknown response length is treated as streaming-capable regardless of its
media type. This protects NDJSON, streaming JSON, gRPC-Web server streams, and
progressive text without maintaining an incomplete content-type allowlist.
Known-length responses remain eligible; Pingora removes the original
`Content-Length` when it applies gzip.

Do not broaden compression to unknown-length responses unless the selected
encoder demonstrably flushes every body chunk and a regression test verifies
incremental delivery.
