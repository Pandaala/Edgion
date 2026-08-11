# AI Gateway Data-Plane Design Review — Buffering, Copies, Structs, Plugins

| Key | Value |
|-----|-------|
| Created | 2026-08-09 |
| Status | completed (living document — update when the fork or pipeline changes) |
| Type | investigation |
| Verified against | Edgion `feature-06-24` working tree; Pingora fork `/Users/caohao/ws2/pingora` branch `edgion`, HEAD `0b6ae51` |
| Review method | Two independent deep-read passes (request path / response path) + orchestrator spot-verification of every copy-count pillar citation |

Every claim carries a `file:line` citation so it can be re-checked. Pingora paths are
relative to the fork checkout (`/Users/caohao/ws2/pingora`); Edgion paths relative to
the repo root. "memcpy" = real byte copy; "refcount" = `Bytes::clone()` (no copy);
"move" = ownership transfer.

---

## 0. Executive summary

Edgion's AI-gateway data plane is built on three custom seams in the Pingora fork
(13 commits ahead of upstream `cloudflare/pingora`):

1. **Request-body capture/replay seam** (`b829a34` + `b1d520e`): a `RequestBodyBuffer`
   trait the proxy tees every downstream body chunk into, with cancellation-safe
   peek/consume replay toward the upstream, typed terminate, upstream framing
   disposition, and a retry predicate.
2. **Async response-body filter with bounded emit + terminate** (`0b6ae51`): the
   `upstream_response_body_filter` hook became async and gained a `ResponseBodySink`
   that can inject extra chunks (1 MiB budget per pump batch) and cleanly end the
   response mid-stream (chunked terminator / H2 END_STREAM, no RST).
3. Supporting transport facts and fixes (headers-EOS, trailers, H1 pool desync, H2
   end-stream watch).

On top of these, Edgion implements:

- **Upstream direction**: `BodyStorage` — the single request-body cache (memory →
  disk spill, logical mutation snapshots, zero-copy replay slices), gated per plugin
  by `BodyRequirement`, plus a non-caching streaming handler chain
  (`RequestBodyStreamHandler`).
- **Downstream direction**: a strictly streaming per-chunk pipeline — private AI
  processor slot (`StreamingResponseProcessor`) → plain processors → read-only
  filter chains — carrying SSE decoding, usage extraction, Anthropic↔OpenAI
  translation, and mid-stream over-quota termination.
- **AI subsystem** (`edgion-gateway/src/ai/`, ~11.5k lines): candidate routing
  (5 algorithms), credential pools with Redis RPM/TPM reservations, bounded
  retry/failover scheduler, batched Redis token quota with atomics-only hot path,
  fixed-point cost accounting, OpenAI-canonical error surface.

**Copy-count bottom line** (details §5):

- Request path, in-memory case: **2 payload memcpys** end to end (H1 chunk
  ownership + capture accumulation); replay to upstream is zero-copy slices.
- Response fast path (no plugins): **2 payload memcpys** (kernel→user read +
  chunk ownership), then pure `Bytes` moves to the downstream socket (vectored
  write, no re-copy).
- Response AI path adds: SSE decoder ~2× copies of `data:` content
  (byte-at-a-time line assembly + field accumulation) plus a per-event
  `serde_json::Value` parse — the dominant cost, already tracked by the
  zero-copy redesign task. The Anthropic route re-renders the stream (1 memcpy)
  and runs a **second** decoder over the translated output.

---

## 1. Pingora fork: request-body seam

### 1.1 Commits

- `b829a34` (2026-07-30) — capture/replay seam: new
  `pingora-core/src/protocols/http/body_buffer.rs` (654 lines today), capture tee in
  `pingora-core/src/protocols/http/{server.rs,v1/server.rs,v2/server.rs}`, replay
  wiring in `pingora-proxy/src/{proxy_h1.rs,proxy_h2.rs,proxy_custom.rs}`.
- `b1d520e` (2026-08-03) — canonical request-body transport interface: typed
  terminate (`RequestBodyAction`), `UpstreamRequestBodyDisposition`,
  `request_retry_allowed`, trailer hook, headers-EOS/trailers-present transport
  facts; mostly `pingora-proxy/src/proxy_trait.rs` (+293) and `proxy_common.rs`
  (+801). Also fixed three pre-existing defects (double-response concat on
  retry-after-commit, post-EOF RST_STREAM misclassification, H1 pool desync on
  incomplete request body).

### 1.2 The `RequestBodyBuffer` trait (`body_buffer.rs:259-359`)

| method | role | key contract |
|---|---|---|
| `async fn write(&mut self, data: &Bytes)` | one call per captured chunk | `Err`/cancelled future poisons the session fail-closed; graceful 413 must NOT come from `Err` here (body_buffer.rs:262-280) |
| `async fn finish(&mut self)` | finalize at downstream EOF | must be idempotent (body_buffer.rs:285-295) |
| `async fn rewind(&mut self)` | reset replay cursor; called before **every** upstream attempt | body_buffer.rs:297-304 |
| `async fn next_chunk(&mut self, max_bytes)` | **pure peek**, no side effects | cancellation-safe: polled inside `select!`, may be dropped mid-disk-read (body_buffer.rs:306-352) |
| `fn consume(&mut self, bytes)` | advance cursor; sync, infallible | committed in the same poll that delivers the chunk (body_buffer.rs:354-358) |

The peek/consume split is the cancellation-safety mechanism:
`RegisteredRequestBodyBuffer` (body_buffer.rs:29-140) tracks `uncommitted: usize`,
commits it synchronously at the start of the next `next_chunk()` before any await
(body_buffer.rs:105-111); a future dropped by a losing `select!` branch re-serves
the same chunk (tests body_buffer.rs:593-633). State machine
`Capturing → Ready → Replaying → ReplayDone` (body_buffer.rs:21-27); `begin_replay`
resets `uncommitted = 0` (body_buffer.rs:90-92). Replay chunks bounded to
`REQUEST_BODY_REPLAY_CHUNK_SIZE = 64 KiB` (body_buffer.rs:19, verified); empty or
oversized impl chunks fail the request (body_buffer.rs:116-130).

The reference `InMemoryRequestBodyBuffer` (body_buffer.rs:364-425) is unused by
Edgion; the pre-existing `FixedBuffer` native retry buffer also lives here
(body_buffer.rs:144-186) and is mutually exclusive with the seam
(v1/server.rs:1156-1161).

### 1.3 Registration and capture tee

Entry points on `HttpSession`: `set_request_body_buffer`
(pingora-core/src/protocols/http/server.rs:649), `set_bodyless_request_replay_buffer`
(server.rs:662), plus `request_body_buffer_registered` / `..._replaying` accessors.

H1 (`v1/server.rs`):
- `early_body_buffer: Option<RegisteredRequestBodyBuffer>` (v1/server.rs:98-100) +
  three fail-closed latches: `early_body_capture_poisoned` (:101-106),
  `early_body_buffer_discarded` (:576-583), `early_body_buffer_released` (:799-813).
- Registration fails closed if: already registered, retry buffering enabled
  (double-send defense), upgrade request, empty body, or any body byte already read
  (:1152-1182). Bodyless variant requires empty downstream body + pre-finalized
  buffer (:1188-1219).
- Capture tee in `read_body_bytes` (:479-514): each chunk teed via
  `buffer.capture(&bytes)` with the poison flag held across the await (:501-505,
  verified); `finish_capture` at body-done/EOF (:487-493, 506-512).
- Replay: `begin_request_body_replay` (:1236-1264) checks the latches;
  `read_body_or_idle` serves `registered.next_chunk()` instead of the socket while
  replaying (:1285-1297); replay EOF triggers `maybe_release_early_body_buffer`
  (:799-813) so an SSE-length response does not pin a spill file.
- Transport-fact overrides: `is_body_done()` false while replaying (:594-604),
  `is_body_empty()` false while registered (:628-634), `downstream_body_done()`
  bypasses the override for keepalive (:606-616), `request_headers_end_stream()` is
  the client-wire fact ignoring the buffer (:645-668).

H2 (`v2/server.rs`) mirrors it (:182-199, tee :485-498) with flow-control released
*before* the cancellable capture await so a cancelled capture cannot leak window
(:477-484); `poll_read_body_bytes` fails while a buffer is registered because
`write` is async (:531-535).

### 1.4 Proxy-loop integration (new `ProxyHttp` hooks)

- `begin_request_body_replay()` runs right after `upstream_request_filter`,
  deliberately before disposition resolution (proxy_h1.rs:137-142); disposition
  coerced by `safe_upstream_disposition` / `apply_upstream_body_disposition`
  (:147-155). Native retry buffering gated on `request_retry_allowed` (:176-178);
  replayed chunks bypass the retry-buffer tee (v1/server.rs:1285-1297).
- Body pump: `read_body_or_idle` → `send_body_to_pipe` (proxy_h1.rs:535-583,
  1019-1117): downstream modules' `request_body_filter`, then the application's
  `request_body_filter_action` on `&mut Option<Bytes>` (:1061-1076), then the
  `Bytes` **moves** into `HttpTask::Body(data, eos)` over the mpsc (:1108-1114);
  upstream task writes with `client_session.write_body(&d)` (:1128-1133).
  Replay-originated errors are attributed gateway-internal (`e.into_in()`,
  :542-548).
- Hooks in `proxy_trait.rs`: `RequestBodyAction::{Continue,Terminate}` (:26-49),
  `request_body_filter_action` (default delegates to legacy filter, :206-219),
  `request_trailer_filter` (:221-260), `upstream_request_body_disposition` — sync
  on purpose, `Ordinary|Bodyless|Streamed` with documented coercions (:51-84,
  262-296), `request_retry_allowed` — live-sampled, must be monotonic (:298-326).
  Terminate travels as a typed outcome through a `biased` `select!`
  (proxy_h1.rs:194-222).
- Custom-connector sessions fail closed on the whole seam
  (proxy_custom.rs:85-93, 118-129, 800-813, 454-479).

### 1.5 Ownership at the fork layer

- H1: socket → session parse buffer (v1/server.rs:516-521) →
  **`Bytes::copy_from_slice(self.get_body(&b))` — the one real memcpy creating the
  owned chunk** (v1/server.rs:496, verified). The same `Bytes` is teed by reference
  into retry buffer and `early_body_buffer.capture(&bytes)` (:498-505) and returned
  to the app by value.
- H2: `request_body_reader.data()` yields an h2-owned `Bytes` — no extra user-space
  copy (v2/server.rs:403, 441).
- The trait passes `&Bytes` on capture (impl chooses copy vs clone) and returns
  owned `Bytes` on replay, which the pump moves (never clones) to the upstream
  writer.

---

## 2. Pingora fork: response-body filter (commit `0b6ae51`)

### 2.1 Hook signature

`upstream_response_body_filter` is now **async** and receives a
`&mut ResponseBodySink` (pingora-proxy/src/proxy_trait.rs:584-596):

```rust
async fn upstream_response_body_filter(&self, _session: &mut Session,
    _body: &mut Option<Bytes>, _end_of_stream: bool,
    _sink: &mut ResponseBodySink, _ctx: &mut Self::CTX)
    -> Result<Option<Duration>>
```

Mutate `body` in place for 1-in-1-out; use `sink` to emit *additional* chunks or
end the response early; `Some(Duration)` still means throttle-delay
(proxy_h1.rs:854-857). The stock downstream-direction `response_body_filter`
remains sync and sink-less (proxy_trait.rs:609-620) — Edgion does not implement it.

### 2.2 `ResponseBodySink` — bounded emit (`response_body_sink.rs`)

- `RESPONSE_BODY_EMIT_BUDGET = 1 MiB`, doc: "Matches the hard cap on
  plugin-initiated outbound response bodies" (:29-31, verified).
- Budget is **per pump batch** (up to `TASK_BUFFER_SIZE = 4` tasks per select-arm
  wakeup, lib.rs:79; `sink.reset_batch()` once per batch — proxy_h1.rs:629,
  proxy_h2.rs:1025).
- `push(chunk)`: empty chunks dropped (an empty downstream body chunk would end the
  response); over-budget push returns `InternalError` — never truncates silently
  (:61-78). `terminate()` is sticky across `reset_batch` (:80-84, 106-111).
- Backpressure comes from the pump structure, not the sink: bounded
  `mpsc::channel::<HttpTask>(4)` (proxy_h1.rs:173-174) +
  `write_response_tasks(...).await` blocking on the downstream socket (:702).
  **Residency is bounded (1 MiB/batch); total emitted bytes over a stream are not**
  (tracked as D6, `todo/response-body-pipeline-hardening.md:142-159`).

### 2.3 Ordering, eos migration, cache coherence

- `drain_emitted_chunks` appends extras after the chunk they were emitted from;
  an eos flag migrates onto the **last** emitted chunk via `migrate_end_of_stream`
  (proxy_cache.rs:1087-1099, 1113-1142; the only refcount `Bytes::clone` on the
  path is here at :1090/:1094).
- Cache is fed the original task plus `peek_extra()` chunks before downstream
  transformation (`cache_task_and_emitted_chunks`, proxy_cache.rs:750;
  proxy_h1.rs:859-864) so a later cache hit is byte-identical.
- Injection is invisible to later processors (Edgion test
  `pg_upstream_response_body_filter.rs:483-502`).

### 2.4 Terminate

Typed outcome `DownstreamRequestOutcome::Terminate` (proxy_common.rs:567-574) —
bypasses retry classification and `fail_to_proxy`.

H1 pump: per batch, if `sink.is_terminated()` after a task the batch drain
**breaks** so unseen same-batch tasks cannot leak to the client
(proxy_h1.rs:648-664, verified — load-bearing comment in code); terminate during
streaming cache readback is a fail-closed `InternalError` (:667-693); otherwise
surviving chunks + injected extras are written, `session.set_keepalive(None)`
(:716), `finish_terminated_response` (:719) → for chunked responses the
`0\r\n\r\n` terminator — the client sees a clean end of stream. A still-committed
`content-length` triggers `warn_response_body_terminate_content_length_leak`
(proxy_common.rs:492-512); Edgion strips CL via `changes_body_length()` (§4.2).

H2: same shape, END_STREAM, deliberately no RST_STREAM after END_STREAM
(proxy_h2.rs:1100-1121). Custom connector: fail-closed (proxy_custom.rs:454-477).
Cache write lock released on terminate (`release_cache_on_terminate`,
proxy_common.rs:406-410).

### 2.5 Chunk ownership

`HttpTask::Body(Option<Bytes>, bool)` moves end to end: upstream reader →
`tx.send` (proxy_h1.rs:1113) → batch `Vec<HttpTask>` (:615-625) →
`h1_response_filter` by value → filter mutates `&mut Option<Bytes>` in place →
`out_tasks` → `write_response_tasks`. No payload copies inside the proxy loop.

---

## 3. Edgion upstream direction: `BodyStorage` and plugin access

Module `edgion-gateway/src/request_body/`: `mod.rs` (facade), `config.rs`,
`state.rs` + `state/enabled.rs` (request-scoped handle), `storage.rs` (1786 lines),
`operation.rs` (scanners; `MAX_BODY_SEGMENTS = 4096`).

### 3.1 Two halves, one allocation

`BodyStorage::new(memory_limit, max_body_size_bytes) → (BodyStorage,
Arc<BodyStorageView>)` (storage.rs:903-932). The `BodyStorage` half is boxed into
Pingora as the `RequestBodyBuffer` impl (session-owned); the view half is installed
into `ctx.request_body` for plugins (state.rs:145-147 → state/enabled.rs:197-200).
Shared inner (storage.rs:240-245): `initial: OnceLock<InitialBody>`,
`materialized: OnceCell<Bytes>`, `mutation: parking_lot::RwLock<MutationState>`,
`max_body_size_bytes`.

### 3.2 Capture state machine and spill

`CaptureStorage`: `Memory(BytesMut) → Disk{NamedTempFile, Arc<File>} →
FinishedMemory(Bytes) | FinishedDisk{..}` + `Poisoned` (storage.rs:882-888).
`write()` (storage.rs:957-1021, verified):
- Physical cap first: `next_len > max_body_size_bytes` →
  `ErrorType::HTTPStatus(413)` (:967-972) — the one typed error the adapter maps
  to a graceful 413 (session_adapter.rs:285).
- Memory path: `buf.extend_from_slice(data)` — memcpy (:975-977).
- First over-threshold chunk: prefix `freeze()` (move) + `create_spill_file`
  writes prefix + chunk into a fresh `NamedTempFile` on `spawn_blocking`
  (:987-997, 1187-1210); later chunks `write_all_at` at `offset = captured_len`
  (:999-1008). The `data.clone()` into the blocking closure is refcount.
- `finish()` publishes `BodySource::Memory(Bytes)` (refcount) or
  `BodySource::Disk{file, len}` into the view's `OnceLock`; idempotent
  (:1024-1039); `NamedTempFile` guard lives in `FinishedDisk` until drop.

### 3.3 Logical snapshot model (mutation without rewriting bytes)

`BodySnapshot { segments: Vec<Segment>, len, sha256: OnceCell }` where `Segment` =
`SourceRange{source, offset, len}` (zero-copy window) or `Inline(Bytes)`
(storage.rs:36-61). Mutations (`clear/append/prepend/remove/replace_literal/
replace_slice/overwrite_body`, storage.rs:466-606) build a new segment list
(adjacent same-source ranges merged; bounded by `MAX_BODY_SEGMENTS`,
storage.rs:194-227) and publish via generation-checked CAS re-validating the
physical cap (storage.rs:270-290). Failed mutation leaves the previous version
readable (test ctx.rs:2049-2064). (Review finding BODY-002.)

### 3.4 Read paths

- `read_all` (storage.rs:292-315): memory → `body.clone()` refcount (:305);
  disk → whole-body materialize into `materialized: OnceCell<Bytes>` via
  `read_disk_body` (`vec![0; len]` + positional read on `spawn_blocking`,
  :1240-1260), memoized. With mutations, linearizes via
  `BytesScanner::with_capacity` (one copy of `len`, :294-300).
- Chunked scanning: `scan_snapshot(_owned)` walks segments in
  `SCAN_CHUNK_SIZE = 64 KiB` steps with a scan-byte budget (storage.rs:22,
  706-757); memory/inline chunks are `bytes.slice(..)` refcounts, disk chunks a
  `read_at_most` Vec copy (:759-799). Disk scans throttled by a global
  `DISK_SCAN_CONCURRENCY = 32` semaphore (:23-28) on `spawn_blocking` (:652-667);
  `CancellationGuard` poisons a dropped offloaded scan (:693-704).
- `snapshot_reader`: owned sequential reader, no materialization (:321-328,
  616-681). `sha256` memoized per snapshot (:60, 608-613).

### 3.5 Replay

`rewind()` pins the **current published snapshot** (post-mutation bytes;
storage.rs:1055-1070); `next_chunk(max)` serves memory/inline as `bytes.slice(..)`
refcounts (:1106-1110, 1131-1135) and disk as `read_at_most` on `spawn_blocking`
with short-read = hard error (:1111-1129); `consume()` validates monotonic
in-bounds advance, else latches `replay_invalid` (:1140-1158).

### 3.6 Byte caps

- **Physical cap** `max_body_size_bytes` (EGC policy, default 32 MiB, config.rs:5):
  at capture (:967-972), at mutation publish (:275-279), pre-computed in
  `replace_*` (:531-546, 592-597).
- **Memory threshold** `memory_buffer_bytes` (default 128 KiB, config.rs:4):
  spill trigger only, clamped ≤ physical cap (config.rs:40).
- **Per-plugin `maxBodySize`**: not a buffer knob — checked by the body gate
  against the already-captured length (body_gate.rs:42-65; doc :13-18).
- **Scan budgets**: per-operation `max_scan_bytes` (storage.rs:723-725) +
  `MAX_BODY_SEGMENTS`.

### 3.7 Concurrency & lifecycle

View reads take an RwLock read only to clone the snapshot Arc (storage.rs:261-268);
mutations take the write lock only to CAS-publish (:280-289). Capture side is
lock-free (fork guarantees serial calls); no async locks on the request path;
disk IO via `spawn_blocking`. Created lazily (`capture_request_body`,
session_adapter.rs:269-275; bodyless owner :211-219). Dropped at replay-EOF +
committed response (v1/server.rs:799-813) or with the session; dropping
`FinishedDisk` deletes the spill file. Plugin view released at the end of the
final response-header chain (`release_late_request_body_view`,
pg_upstream_response_filter.rs:74-82); `modified_len_for_framing` survives release
so retries rebuild identical `Content-Length` (state/enabled.rs:181-191).

### 3.8 Plugin access APIs

- **Declaration**: `BodyRequirement` on the plugin CR wraps the plugin in
  `BodyGatedRequestFilter` (`Conditional(BodyGate(inner))`, body_gate.rs:28-30):
  evaluates run/skip conditions, buffers to the gateway-wide cap
  (`read_request_body(Some(cap))`, or `ensure_request_body` for non-materializing
  plugins — the HMAC streaming-hash case, body_gate.rs:107-116), then applies the
  per-plugin `maxBodySize` to the captured length (:117-121). Failure policy:
  `NotBuffered`/physical-cap/`StorageFailure` unconditionally fail-close;
  `TooLarge/Timeout/Incomplete/Unimplemented` honor `on_read_failure`
  (body_gate.rs:175-262).
- **`PluginSession` facade**: every body method defaults to
  `Err(BodyReadError::Unimplemented)` — the "no capture capability" sentinel
  (traits/session.rs:438-534); taxonomy `Unimplemented | NotBuffered |
  BufferLimitExceeded | TooLarge | Incomplete | StorageFailure | Timeout`
  (:26-71).
- **Real adapter** (`PingoraSessionAdapter`): `ensure_request_body`
  (session_adapter.rs:1083-1115) runs the admission predicate (:584-622) —
  no declared body → PassThrough; missing snapshot / past request stage /
  WebSocket / gRPC → refuse; else capture. `capture_request_body` (:231-306):
  declared CL > cap → immediate 413 + keepalive off; `Expect: 100-continue` →
  `write_continue_response()` then strip (the fork-documented deadlock,
  body_buffer.rs:233-247); create `BodyStorage`, register, drain to EOF counting
  against the cap; error mapping ReadTimedout→Timeout, HTTPStatus(413)→
  BufferLimitExceeded, InternalError→StorageFailure (:282-292).
- **Streaming (non-caching) chain**: `RequestBodyStreamHandler`
  (traits/request_body_stream.rs:1-80) installed per request, driven by
  `request_body_filter_action` before the chunk goes upstream
  (pg_request_body_filter_action.rs:20-70); handlers mutate `&mut Option<Bytes>`
  in place, may return `ChunkVerdict::Terminate{status,body,headers}`; once any
  handler is installed the monotonic latch flips disposition to `Streamed` and
  `request_retry_allowed` to false (proxy_http/mod.rs:145-169). Mirror tee runs
  after the handlers (post-mutation chunks).

### 3.9 Consumers

- **DSL `req.body`**: opaque `Value::Body` token; metadata check only at builtin
  eval (dsl/lang/builtins.rs:1382-1409); text/JSON/regex/digest run through the
  shared scanners in `dsl/lang/body_scanners.rs` over the view — uniform over
  memory and disk; `resp.body` rejected statically and at runtime (:1401-1403).
- **ExtProc**: request BUFFERED reads the shared cache and copies into protobuf
  (`body.to_vec()`, ext_proc/plugin.rs:~364 — the largest avoidable per-consumer
  copy); STREAMED installs `ExtProcStreamedBodyHandler` exchanging each chunk over
  the request-scoped gRPC stream in place (streamed.rs:1-60). Response BUFFERED/
  STREAMED rejected at validation (plugin_configs/ext_proc.rs:251-258).
- **Wasm**: buffer-mode re-read of the shared cache (cache hit; wasm/plugin.rs:
  243-263); `NotBuffered` unconditionally fail-close (:264-274). Response body:
  per-chunk read-only.
- **ai_proxy**: requires declared body, reads fully (`read_request_body(None)`,
  ai_proxy/plugin.rs:133-142) and parses into the canonical OpenAI request.
- **Framing sync**: `sync_body_framing_headers` strips `Transfer-Encoding` and
  sets `Content-Length: <new len>` only when a mutation happened
  (pg_upstream_request_filter.rs:70-92); same hook clears `in_request_body_stage`
  (:58-65).

### 3.10 Policy resolution (invariant 5)

Schema `RequestBodyConfig` (edgion-resources/src/resources/
edgion_gateway_config.rs:125-145); `ResolvedRequestBodyConfig::resolve` parses
human-readable sizes once and clamps memory ≤ body (request_body/config.rs:12-44).
The EGC conf handler publishes one immutable
`EdgionGatewayConfigSnapshot { configs, request_body_configs }` through a
`LazyLock<ArcSwap<..>>` (config/edgion_gateway/conf_handler_impl.rs:11-52);
`pg_new_ctx` does one ArcSwap load + bounded map lookup and copies the compact
`Copy` struct into the ctx (conf_handler_impl.rs:77-89, pg_new_ctx.rs:16-17).
No parsing, no locks on the request path.

---

## 4. Edgion downstream direction: streaming response pipeline

Edgion implements `upstream_response_filter`, `response_filter`,
`upstream_response_body_filter`, `logging` — not the downstream-direction
`response_body_filter` (proxy_http/mod.rs:183-223).

### 4.1 Header stage (`pg_upstream_response_filter.rs:96-471`)

1xx forwarding → stale local-reply clear → status/outlier bookkeeping → **AI retry
arbitration** + 429 normalization + retry budget (:150-227) → non-AI retry-on-status
(:228-293; retried responses never reach processors) →
`StreamingResponseProcessor::on_response_header` on the final response only
(:295-300) → `classify_response_stream_kind`: exact `text/event-stream` ⇒ `Sse`,
and **SSE disables response compression** (`adjust_level(0)`) so the compressor
cannot buffer/stall events (:302-315; content_type.rs:28-41) → session persistence →
sync operator plugin chains in onion order route → backend → gateway → global,
gated on `response_stage_settled` (:398-454) → `enforce_stream_processor_framing`
strips `content-length` when any processor `changes_body_length()` (:456-460,
checked at both header boundaries) → release late request-body view (:462).

### 4.2 Body stage (`pg_upstream_response_body_filter.rs:98-161`), per chunk

1. TTFT stamp on first non-empty chunk (:106, 167-180).
2. `run_stream_processors` (:50-90) — the **writable** pipeline:
   `ctx.ai_stream_processor` (private AI slot) first, then `ctx.stream_processors`
   in installation order. `mem::take` prevents mid-stream installs (:41-45).
   Terminate guards before the AI slot, before the plain chain, and after each
   plain processor (:70-85).
3. Read-only `UpstreamResponseBodyFilter` chains: route → gateway → global
   (:117-158), sync, `&Option<Bytes>`, see post-mutation bytes, return max
   throttle delay; **not terminate-aware** (D8). Implementors today:
   `bandwidth_limit` (delay only) and `wasm` (read-only observation).

Writable contract (`traits/streaming_response_processor.rs:36-87`): async
per-chunk; mutate = transform (visible downstream in chain); `sink.push` = inject
(invisible); `sink.terminate()` suppresses **all** further calls including eos —
settlement must live in the `logging` phase; terminating/length-changing
processors MUST override `changes_body_length()`; outbound calls must self-bound
with `tokio::time::timeout`. Not operator-facing — only request-stage plugins
install processors.

### 4.3 Local reply & keepalive

Single local-reply write point `pg_response_filter.rs` (:133-146): sets
`local_reply_written`, `set_keepalive(None)`, writes header+body, returns a
sentinel `InternalError` that `pg_fail_to_proxy.rs` recognizes. Known limitation:
forfeits H1 keepalive after every response-stage local reply
(`todo/http1-response-local-reply-keepalive/`, planned P3 via
`FailToProxy.can_reuse_downstream`).

### 4.4 SSE decoder (`edgion-gateway/src/sse/mod.rs`)

Bounded, incremental, push-based `SseDecoder` (:53-98): BOM state, `pending:
Vec<u8>` (current incomplete line), sticky `degraded`/`malformed`, current-event
accumulators, `retained_event_bytes`, CRLF-across-chunks state. APIs: `feed`
(batch, effectively test-only) and `feed_each(chunk, visitor)` (:135-147) — one
owned `SseEvent {event, data, id}` at a time.

Parsing is a byte-at-a-time state machine: every payload byte is pushed into
`pending` (:226, verified — first data copy), lines move out via `mem::take`
(:257), `consume_line` UTF-8-validates then `cur_data.push_str(value)` (second
copy of data content, :281); `dispatch` moves `cur_data` into the event (:294).
Consumers add a per-event `serde_json` parse — the dominant cost
(`todo/sse-decoder-zero-copy-stream.md:29-37`).

Bounds: `DEFAULT_MAX_PENDING_EVENT_BYTES = 1 MiB` (:37, verified), sized for
"fake streaming" providers; ~1-2 MiB resident per decoder, ×2 on the Anthropic
path (two decoders); no aggregate cross-stream bound — accepted because reaching
it requires an operator-configured provider (:9-36).

Failure modes: **degraded** (oversized incomplete event) permanently stops parsing
and frees buffers; **malformed** (invalid UTF-8 in a field line) drops the
poisoned event and increments a cumulative completed-event count. Protocol
translators still use the sticky signal to fail closed. D1b is fixed on the
OpenAI quota path: one malformed event is tolerated, while the second fails
closed under the streaming quota Reject gate; valid events do not reset the
count.

### 4.5 AI module (`edgion-gateway/src/ai/`, 17 files ~11.5k lines)

| File | Role |
|---|---|
| `error.rs` | `AiProxyError` (10 variants) → fixed OpenAI-shaped error table (:23-86); no internal detail leaks |
| `attempt_plan.rs` | `ReadyAiAttemptPlan` — immutable per-request forwarding decision incl. `terminate_on_over_limit` (:94; true only for `onExceeded: Reject`, ai_proxy/plugin.rs:453) |
| `routing.rs` | 5 selection strategies: Weighted, Priority, LowestLatency (30 s rolling p50 TTFT + exploration), LowestUsage, CostWeighted (:115-318); integer-only, fails closed on overflow |
| `attempt.rs` | Bounded scheduler: ≤16 services × ≤16 credentials, ≤8 dispatches / ≤7 retries; credential-rotation-before-service-failover (:8-11) |
| `attempt_runtime.rs` | Per-attempt admission (circuit breaker → outlier → Redis per-credential RPM/TPM reservation, `edgion:aic:` keys); 2xx ≠ success until EOS latch (:104-112); settlement Release/Reconcile/Retain (:470-490); `settled = eos \|\| gateway_terminated()` (:428) |
| `adaptive.rs` | Lock-free per-(route,rule,service,model) telemetry: 1 s ring buckets, ArcSwap snapshots, 1 Hz ticker |
| `circuit_breaker.rs` | Per-service in-flight cap (RAII semaphore) — concurrency, not failure-rate |
| `credential_outlier.rs` | Passive per-credential ejection + Retry-After cooldowns (≤5 min) |
| `dispatch.rs` | Builds each attempt: credential selection, model rewrite or `prepare_anthropic_request`, auth-header hygiene (strips authorization/x-api-key/anthropic-*, :369-405), provider URI mapping, installs the response processor chain (:427-455) |
| `usage.rs` | `UsageExtractionProcessor` — see §4.6 |
| `quota/mod.rs` | Atomics-only hot path (`add_output_units` fetch_update + flush-boundary notify, :102-116; sticky lock-free `over_limit` latch :147-156); one bounded worker per quota'd request does all Redis I/O; a mid-stream flush that reports over-limit latches immediately (:279-284) |
| `quota/runtime.rs` | Redis tumbling windows, Identity/IdentityModel scopes, hashed slot-stable `edgion:aiq:` keys, two-phase admission + cumulative reconciliation, TTL leases; `onExceeded` Reject/Throttle/Downgrade; `onRedisFailure` allow/deny |
| `service_store.rs` | ArcSwap runtime index from the `EdgionAIService` CR; Arc-preserving recompiles; invalid update deactivates the service |
| `anthropic/request.rs` | OpenAI→Anthropic request translation: 13-field whitelist, unsupported params fail closed, mandatory `max_tokens` |
| `anthropic/response.rs` | Anthropic→OpenAI translation, non-streaming + full SSE state machine (:381-596); `changes_body_length()` always true (:650-652); fails closed (sealed) on degraded/malformed (:367-369); owns quota + termination on Anthropic routes (:686-696) |

Notable: **no tokenizer in the repo** — provider-reported usage or ceil(chars/4)
estimate only (dispatch.rs:479). Exactly one processor owns quota per provider
(dispatch.rs:443-445).

### 4.6 Mid-stream over-quota termination (end to end)

1. Request stage: ai_proxy admits quota, builds the plan
   (`terminate_on_over_limit` ⇔ `onExceeded: Reject`).
2. `ai::dispatch` composes `[AnthropicResponseProcessor?] +
   UsageExtractionProcessor` into the private `ctx.ai_stream_processor` slot
   (dispatch.rs:72, 427-455).
3. Per chunk: SSE events decoded → `quota.add_output_units(approximate units)` →
   worker flush at token boundaries → Redis says over → worker latches
   `mark_over_limit` mid-stream (quota/mod.rs:279-284).
4. `on_chunk` termination gate — all of `requested_streaming &&
   terminate_on_over_limit && quota.is_some()` (usage.rs:864-866, verified;
   rationale: never truncate a non-streaming completion, never override operator
   Throttle/Downgrade) — then `terminate_over_quota` on the latch, or
   `terminate_quota_unenforceable` when the decoder degraded (the D1 fix: a
   degraded decoder can never grow the accumulator, so enforcement would be
   silently off; usage.rs:867-885, verified).
5. `terminate_ai_stream` (usage.rs:142-156): `sink.push` of a static SSE error
   event **led by `\n\n`** to close any partial upstream event (D3 fix, :23-52;
   body reuses the 429 admission error), `sink.terminate()`,
   `mark_gateway_terminated()`, access-log error code.
6. Fork: batch break → surviving chunks + injected event written → chunked
   terminator / END_STREAM → typed `Terminate`. `changes_body_length()` returns
   the same triple conjunction so CL is stripped iff termination is reachable
   (usage.rs:811-813).
7. Settlement: no eos fires by design; credential reservation reconciles via the
   `gateway_terminated` latch (D2 fix, attempt_runtime.rs:428); user quota bucket
   reconciles in `pg_logging.rs:41-43, 191-215`.

### 4.7 Streaming matchers

Request-side only (`plugins/http/dsl/lang/body_scanners.rs`; used from the DSL VM
over the BodyStorage view — **no response-side matcher exists today**):
- `Md5Scanner`/`HmacSha256Scanner`: incremental digests, no payload copy (:25-63).
- `StreamingRegexScanner`: precompiled `regex_automata` dense DFA stepped
  **byte-at-a-time across chunk boundaries** — DFA state *is* the lookbehind, no
  window buffer, no per-chunk copy; early-exit on first match (:130-167).
- `JsonPathScanner`/`JsonSelectScanner`: streaming serde over a channel to a
  blocking thread; each chunk copied (`to_vec`, :340/:371); 32-permit semaphore.

`response_rewrite` is status/headers only. The only response-body transformation
machinery is the private `StreamingResponseProcessor` layer.

---

## 5. Copy-count summary

### 5.1 Request path (H1 downstream, buffered body, one plugin read, one attempt)

| # | step | operation | cost | citation |
|---|------|-----------|------|----------|
| 0 | socket → parse buffer | `body_reader.read_body` | kernel→user | v1/server.rs:516-521 |
| 1 | parse buffer → owned chunk | `Bytes::copy_from_slice` | **memcpy #1** | v1/server.rs:496 |
| 2 | tee into capture | `&Bytes` borrow | none | v1/server.rs:501-505 |
| 3 | capture accumulate | `BytesMut::extend_from_slice` | **memcpy #2** | storage.rs:975-977 |
| 4 | finalize | `freeze()` + refcount publish | move/refcount | storage.rs:1029-1032 |
| 5 | plugin `read_all` (memory) | `body.clone()` | refcount | storage.rs:304-305 |
| 6 | replay `next_chunk` ≤64 KiB | `bytes.slice(..)` | refcount | storage.rs:1106-1110 |
| 7 | pump → upstream task | move through mpsc | move | proxy_h1.rs:1061-1114 |
| 8 | upstream write | `write_body(&d)` | user→kernel | proxy_h1.rs:1128-1133 |

**In-memory total: 2 user-space memcpys.** H2 downstream removes memcpy #1
(h2-owned `Bytes`). Spill case swaps memcpy #2's tail for: one file write per
chunk, one full file read for a materializing consumer (memoized), one 64 KiB
file read per replay chunk — copies traded for bounded memory. Per-consumer
extras: ExtProc BUFFERED `to_vec()` into protobuf (doubles that plugin's peak).

### 5.2 Response fast path (no processors, no body plugins)

Socket read (kernel→user) + `Bytes::copy_from_slice` chunk ownership
(v1/client.rs:455-459, verified) → moves all the way to the vectored chunked
write (`v1/body.rs:1018-1042` — size header + payload + CRLF chained, no payload
re-copy). **Pure `Bytes` passthrough after the two ingest memcpys.** H2 is better
on both edges (refcounted `Bytes` in, `send_data(Bytes)` out).

### 5.3 Response AI path (SSE + quota + bandwidth_limit)

Transport steps as §5.2, plus per chunk/event:

| step | operation | cost | citation |
|------|-----------|------|----------|
| char counting scan | byte-wise scan | none | usage.rs:699, 440-475 |
| SSE line assembly | `pending.push(byte)` per byte | **memcpy of payload** | sse/mod.rs:226 |
| field accumulation | `cur_data.push_str` | **memcpy of `data:` content** | sse/mod.rs:277-285 |
| per-event JSON | `serde_json::from_slice::<Value>` | **alloc + parse (dominant)** | usage.rs:704 |
| quota accounting | atomics | none | quota/mod.rs:102-116 |
| read-only chain | `data.len()` only | none | bandwidth_limit/plugin.rs:81-93 |
| terminate injection | static refcount clone | none | usage.rs:150 |

Anthropic route additions: translated stream rendered into a fresh `Vec<u8>`
replacing the chunk (anthropic/response.rs:356-378) and a **second decoder pass**
over the translated output — documented double cost (sse/mod.rs:28-30).

The forwarded wire bytes are **never copied** by the observational (OpenAI)
processor — decoding happens on a side path; the chunk `Bytes` itself moves
through untouched. This matches agentgateway's passthrough design point (side
`BytesMut` decode buffer) — see `02-agentgateway-comparison.md` §efficiency.

---

## 6. Invariant cross-check (CLAUDE.md "HTTP body handling invariants")

| Invariant | Status | Evidence |
|---|---|---|
| 1. Only the request body may be cached | Holds — response side has only bounded plugin-scope aggregations (usage 1 MiB non-streaming cap, usage.rs:21/723-739; Anthropic bounded input, anthropic/response.rs:313/833) |
| 2. Upstream response always streaming | Holds — per-decoder bounds + trait prohibition (streaming_response_processor.rs:16-19). Zero-copy/auditing work will need the wording extended to a *bounded window* |
| 2a. DSL `resp.body` rejected at validation | Holds — plugin_configs/dsl.rs:169-172; bytecode detection edgion-dsl/src/bytecode.rs:212-213 |
| 2b. ExtProc response BUFFERED rejected | Holds — plugin_configs/ext_proc.rs:140-144, 252-258 |
| 3. Outbound aggregation behind hard 1 MiB cap | Holds; sink emit budget deliberately matches it (response_body_sink.rs:29-31) |
| 4. Fork-only build, unconditional seam | Holds |
| 5. Policy resolved pre-hot-path | Holds — ArcSwap snapshot + ctx copy (conf_handler_impl.rs:77-89) |

---

## 7. Open gaps registry (consolidated)

Moved task files now live under `tasks/ai-gateway/`. Cross-referenced state:

**Response pipeline / termination** (from `todo/response-body-pipeline-hardening.md`,
still open): D5 (sealed Anthropic stream drains upstream forever, can never terminate), D6 (emit budget:
no per-response total / chunk-count cap), D7 (latent cache-feed panic class),
D8 (read-only chains not terminate-aware, no eos after terminate), D9 (emitted
chunks bypass RangeBodyFilter — latent fork hazard, unreachable from Edgion),
D10 (terminate tears down H1 keepalive unnecessarily), D11 (no config-time
rejection of cache+Reject; client-authorable quota identity), D12 (cache-stored
injected content must be request-invariant). D1/D1b/D2/D3 are fixed (closed decisions
in `skills/04-review/ai-quota/`).

**SSE decoder**: zero-copy slice-chain redesign is design-phase only
(`todo/sse-decoder-zero-copy-stream.md`); it also carries the windowed
content-auditing prerequisite (hold-and-release windows) and the Anthropic
double-decode elimination opportunity.

**Request seam**: ext_proc Envoy parity P1-P3 (`todo/ext-proc-envoy-parity.md`);
`request_trailer_filter` unimplemented in Edgion (trailers dropped — revisit if
gRPC body inspection is ever admitted, session_adapter.rs:617-619); spill-boundary
latency step if operators raise the memory threshold (storage.rs:987-997);
`read_disk_body` materialization is unthrottled by the scan semaphore
(storage.rs:1240-1260).

**Docs debt**: `streaming_response_processor.rs:4` cites a nonexistent spec file
(`docs/superpowers/specs/2026-08-03-ai-gateway-phase1-design.md`); AI-3 subtask
checkboxes are stale relative to shipped code (see
`archive/completed-tasks/ai-gateway-resilience-and-routing/`).
