# core-01: A hostile HTTP/2 upstream can deliver a response `HeaderMap` at its growth limit, so the next header write panics

- **Category**: reachable-panic (static reading plus an algorithm model; not reproduced end to end)
- **Severity**: Low with trusted backends; Medium for untrusted, third-party, or reflecting HTTP/2
  backends (provisional)
- **Scope**: cross-module (Pingora fork upstream h2 client + Edgion/Pingora response writers)
- **Trigger source**: upstream (backend) server, or a client through a reflecting HTTP/2 backend
- **Status**: blocked pending an upstream Pingora fix for response header insertion at capacity; found 2026-11-12 during the wasm-01 reviews
- **Location**:
  - `../pingora/pingora-core/src/connectors/http/v2.rs:699` (upstream h2 client `Builder::new()`
    sets no `max_header_list_size`, so h2's 16 MiB local default applies, `h2-0.4.20/src/codec/framed_read.rs:22`)
  - `h2-0.4.20/src/frame/headers.rs:951` (HPACK decode uses `try_append`; on `MaxSizeReached` it
    sets `is_over_size` and the client refuses the stream, `proto/streams/recv.rs:221-238`, so a map
    is delivered only if the write that would fail never happens inside the decoder)
  - `../pingora/pingora-core/src/protocols/http/v2/client.rs:571` (`ResponseHeader::from`) and
    `../pingora/pingora-proxy/src/proxy_h2.rs:1463` (clone): both keep the map's danger state, since `HeaderMap` derives `Clone`
  - `../pingora/pingora-http/src/lib.rs:898,900` (fork `insert_header_value`, panicking `HeaderMap::insert`)
  - First writers on a plain route: Edgion's server header insert
    (`edgion-gateway/src/runtime/server/server_header.rs:55`, reached from
    `routes/http/proxy_http/pg_upstream_response_filter.rs:255`) and Pingora's unconditional `Date`
    insert when writing the response downstream (`pingora-core/src/protocols/http/v1/server.rs:1192`,
    `pingora-core/src/protocols/http/v2/server.rs:1084`), so
    disabling server headers does not help. In-cap ExtProc/Wasm response writes and
    `PendingHeaderOp::apply` (`ctx.rs`) are further writers.

## Summary
`http` 1.5.0 `HeaderMap` insert/append/entry calls run `try_reserve_one` first, even for an existing
name, and fail when the map is in the displacement-danger (Yellow) state at raw capacity 32,768 with
at least 6,554 distinct names, or at 24,576 names. Green maps hash with unkeyed FNV truncated to 15
bits, so about 130 crafted names can force the Yellow state. The downstream server bounds the request
header list (Pingora's `DEFAULT_MAX_HEADER_LIST_SIZE`, 64 KiB, `protocols/http/v2/server.rs:71,86`),
but the upstream h2 client does not. A hostile HTTP/2 backend can therefore return a response whose
map, together with its danger state, is already at the limit: about 6,555 crafted names (about
236 KB decoded) whose last decoded name triggers the Yellow state, or exactly 24,576 names chosen so
that no intermediate insert turns the map Yellow. Pingora's `validate_response_header` only checks
Content-Length against Transfer-Encoding. The first write on that response then panics the request
task.

A client-triggerable variant exists when an HTTP/2 backend reflects client-chosen names into
response headers (httpbin-style `/response-headers`): about 6,555 query-supplied names fit in the
64 KiB downstream header list.

## Evidence and gaps
- Mechanism traced by the R3 reviewer on 2026-11-12 from source, with an exact Python model of the
  `http` 1.5.0 growth algorithm using abstract 15-bit hashes: 6,553 names leave the map writable,
  6,554 or more make the next write fail. No real names, h2 traffic, or gateway run were used.
- HTTP/1 upstreams are capped at 256 headers (`pingora-core/src/protocols/http/v1/client.rs:274`,
  `MAX_HEADERS`). HTTP/2 upstreams are used for `appProtocol` h2c
  (`edgion-gateway/src/backends/peer_builder.rs:53`) and gRPC.
- Upstream trailers decode the same way; no panicking writer on upstream trailers was found
  (ExtProc trailers use `try_*`; plain `insert` calls on trailers build fresh maps).
- The process survives (`panic = "unwind"`); the affected downstream connection or request is
  dropped and its access log is likely skipped.
- The wasm-01 caps (ExtProc 256 set entries per mutation, Wasm 1,024 writes per phase) bound plugin
  input, not a map that already arrives at the limit.

## Suggested direction
1. Reproduce: a scripted HTTP/2 upstream returning a crafted Yellow map with at least 6,554 names
   (or exactly 24,576 filtered names), followed by the normal response path. Sending more than the
   limit makes h2 refuse the stream instead.
2. Fix options:
   - Edgion side: reject an upstream response whose `headers.keys_len()` exceeds a bound (for
     example 4,096) at the top of `upstream_response_filter`, before the first write.
   - Fork side: set the upstream h2 client's `max_header_list_size` (for example 64 KiB, like the
     downstream server; Edgion's own outbound reqwest client defaults to 16 KiB with 64 KiB as the
     configurable maximum, `edgion-common/src/link_sys_core/http_client.rs:19,22`). This changes upstream-inherited fork code
     (`connectors/http/v2.rs` `Builder::new()`), so weigh it against the fork's "Ownership and fix
     policy" in `../pingora/AGENTS.md`.
3. Record the outcome in `skills/04-review/h2-grpc/` and link it from the
   [ExtProc](../../skills/04-review/ext-proc/regression-guards/header-mutation-operation-cap.md) and
   [Wasm](../../skills/04-review/wasm/regression-guards/header-write-cap-per-phase.md) cap pages and
   `tasks/todo/header-map-insert-max-size-residuals.md`.
