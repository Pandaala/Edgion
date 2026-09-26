# h2 Trailer Pseudo-Header Decoder Remedy

## Status

Superseded on 2026-08-27. The candidate below was validated but rejected as a
maintenance policy: Pandaala will not vendor or locally fork h2 for this
low-frequency upstream defect. This file is retained only as technical evidence
for evaluating a future upstream release.

## Security finding

HTTP/2 trailers must not contain pseudo-headers. h2 0.4.16 and 0.4.19 fully
decode a terminal trailer block, retain pseudo-fields separately, then queue
only its ordinary `HeaderMap` fields. Consequently `:status: 200` followed by
`x-trailer: 1` is surfaced to Pingora as a legitimate nonempty trailer map.

The affected stock receive transition is
`src/proto/streams/recv.rs::recv_trailers`. Stock h2 0.4.16, 0.4.19, and the
recorded upstream baseline call `frame.into_fields()` without checking
`frame.is_over_size()` or `frame.pseudo()`. The historical candidate below
demonstrated the required handoff behavior.
This is a decoder defect, not something a Pingora wire observer can safely
decide from raw HPACK bytes.

## Implemented decoder patch

Apply this after h2 has completely decoded the header block, but before it
closes the receive side or queues an `Event::Trailers`. Full HPACK decoding
still occurs first, preserving connection-level dynamic-table state.

```diff
diff --git a/src/proto/streams/recv.rs b/src/proto/streams/recv.rs
@@ pub fn recv_trailers(
     frame: frame::Headers,
     stream: &mut store::Ptr,
 ) -> Result<(), Error> {
+    if frame.is_over_size() {
+        tracing::debug!(
+            "stream error PROTOCOL_ERROR -- recv_trailers: \\
+             headers frame is over size; stream={:?}",
+            stream.id,
+        );
+        return Err(Error::library_reset(stream.id, Reason::PROTOCOL_ERROR));
+    }
+
+    let (pseudo, trailers) = frame.into_parts();
+
+    if pseudo != frame::Pseudo::default() {
+        proto_err!(stream: "recv_trailers: pseudo-header in trailers; stream={:?}", stream.id);
+        return Err(Error::library_reset(stream.id, Reason::PROTOCOL_ERROR));
+    }
+
     stream.state.recv_close()?;
```

The size check precedes pseudo inspection because h2 intentionally stops
retaining decoded fields once the header-list limit is reached. Without it, a
large pseudo-header can make `frame.pseudo()` appear empty. All six pseudo
fields are then checked, so the same source is safe for client and server
peers. The stream-level `PROTOCOL_ERROR` follows h2's existing malformed
trailer handling and leaves the connection available for other streams unless
other protocol state independently requires closure.

## Historical executable evidence

A temporary complete h2 0.4.19 checkout and Pingora adapter were tested
together before the vendoring proposal was withdrawn. No copy or path patch is
retained in the fork.

| h2 source | Pseudo-only trailer + same-burst NO_ERROR reset | Mixed pseudo + regular trailer + same-burst NO_ERROR reset |
|---|---|---|
| Stock 0.4.16 | Required contract fails on unwatched IO | Required contract fails on unwatched IO |
| Stock 0.4.19 | Required contract fails on unwatched IO | Required contract fails on unwatched IO |
| Historical patched 0.4.19 decoder plus Pingora adapter | Permanent error in async, poll, and direct APIs, with and without reset | Permanent error in async, poll, and direct APIs, with and without reset |

The h2 trailer suite covers client and server receive directions, all six
pseudo-header kinds, pseudo-only and mixed blocks, ordinary and pseudo-only
oversize, and fragmented CONTINUATION. Pingora's product matrix covers
pseudo-only and mixed blocks across async body, poll body, and direct trailer
reads, each with and without a same-burst reset. The terminal-HEADERS latch is
still required because a reset can consume h2's public decoder error event.

## Upstream adoption gate

1. Confirm a released upstream h2 contains the decoder-level remedy.
2. Upgrade it through the normal dependency declaration without a path patch.
3. Confirm h2 regression tests cover pseudo-only, mixed pseudo-plus-regular,
   oversized pseudo-only, ordinary oversized, and fragmented CONTINUATION
   terminal blocks. Run Pingora async, poll, direct-trailer-first, and cache
   negative tests for pseudo-only and mixed trailers, each with and without a
   same-burst reset.
4. Obtain an independent review before unblocking watcher simplification.
