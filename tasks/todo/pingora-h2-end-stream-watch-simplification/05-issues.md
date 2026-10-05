# Open Issues

## H2-DECODER-TRAILER-PSEUDO-001 — accepted upstream limitation

**Status:** local finding closed on 2026-09-01 as an accepted tradeoff; the
upstream dependency blocker remains deferred and no local decoder fork is
maintained. See
`skills/04-review/h2-grpc/regression-guards/h2-trailer-decoder-upstream-limitation.md`.

**Finding:** h2 0.4.16 and 0.4.19 accept a terminal trailer block that begins
with a pseudo-header and then contains regular trailer fields. The pseudo-field
is not exposed in the resulting `HeaderMap`, so Pingora receives a successful
nonempty trailer result. A same-burst `RST_STREAM(NO_ERROR)` can additionally
hide the decoder's stream error for pseudo-only trailers.

**Impact:** any watcher simplification that accepts `Some(nonempty)` as a valid
trailer result can accept a malformed response and potentially admit it to
cache. A raw-wire observer cannot prove the absence of a discarded pseudo-field
without duplicating HPACK decoding.

**Evidence:** stock 0.4.16 and 0.4.19 fail the pseudo-only and mixed decoder
contracts. A candidate patch proved the decoder-level remedy, but maintaining a
complete vendored h2 tree was rejected. See
[04-h2-decoder-remedy.md](04-h2-decoder-remedy.md).

**Required resolution:** wait for an upstream h2 release that rejects oversized
blocks and all pseudo-fields in trailer blocks before any trailer event is
queued. Then enable the core contracts and extend them through poll, proxy
cache, and same-connection reuse paths before reopening the watcher
implementation gate.

**Non-resolution:** raw length checks, accepting only empty blocks, accepting
only nonempty maps, or a parallel HPACK parser are not accepted fixes.
