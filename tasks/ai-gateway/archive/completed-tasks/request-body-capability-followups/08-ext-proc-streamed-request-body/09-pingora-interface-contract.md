# Pingora Request-Body Transport Interface — Consumer Contract

> Audience: whoever implements the Edgion side against this interface.
> This is the **as-built** contract. The design document
> (`05-pingora-canonical-interface-design.md`) records intent; three rounds of
> deep review changed several behaviors after it was written. Where the two
> disagree, **this document wins**.
>
> Fork: `Pandaala/pingora`, branch `edgion`. Edgion pulls it by branch
> (workspace `Cargo.toml:124-129`), so a push to that branch reaches Edgion on
> the next lock update.

## 1. What you get

Four `ProxyHttp` trait methods, two `Session` fact queries, two public types.
Everything has a default that preserves pre-existing behavior; an application
that implements none of it is unaffected except for the shared-path deltas in
§6.

### 1.1 Types (`pingora_proxy`, also in `prelude`)

```rust
pub enum RequestBodyAction { Continue, Terminate }

pub enum UpstreamRequestBodyDisposition { Ordinary, Bodyless, Streamed }
```

### 1.2 Trait methods

```rust
// Per body event. Default delegates to `request_body_filter` and returns Continue.
async fn request_body_filter_action(
    &self, session: &mut Session, body: &mut Option<Bytes>,
    end_of_stream: bool, ctx: &mut Self::CTX,
) -> Result<RequestBodyAction>;

// At most once per downstream request, only when real trailer fields were observed.
async fn request_trailer_filter(
    &self, session: &mut Session, ctx: &mut Self::CTX,
) -> Result<RequestBodyAction>;

// Sync. Queried once per upstream attempt, before upstream request headers are written.
fn upstream_request_body_disposition(
    &self, session: &Session, ctx: &Self::CTX,
) -> UpstreamRequestBodyDisposition;

// Sync. Queried live at four retry decision points.
fn request_retry_allowed(&self, session: &Session, ctx: &Self::CTX) -> bool;
```

### 1.3 Session fact queries (`pingora_core::protocols::http::Session`)

```rust
pub fn request_headers_end_stream(&mut self) -> Option<bool>;
pub fn request_trailers_present(&mut self) -> Option<bool>;
pub fn response_body_finished(&self) -> Option<bool>;
```

`None` means the session type cannot supply the fact (`Custom`, `Subrequest`).
**Treat `None` as "this capability is unavailable" and fail closed** — the same
policy as the stock-build rejection.

## 2. Obligations you must honor

These are contracts the proxy cannot enforce for you. Violating them is either
a hard failure or a silent correctness bug.

| Obligation | Enforcement if violated |
|---|---|
| Before returning `Terminate`, **finish the downstream response yourself** — write a local reply, or observe an already-committed response and abandon. Pingora never writes a response on the terminate path. | Diagnostic `warn!` only; the client gets a bare close. The proxy defensively calls `finish_body()` so a written-but-unfinished body still reaches the client. |
| `Bodyless` means **no downstream body will arrive**. | **Hard fail-close**: real body bytes under `Bodyless` return a typed `InternalError` → 500. Both pumps. |
| `request_retry_allowed` must be **monotonic** — once `false`, never `true` again for that request. | Unchecked. A non-monotonic predicate produces undefined retry behavior. |
| Do not pre-read the downstream request body outside the pump if you need `request_trailer_filter`. | Silent: the hook never fires (§4). |

## 3. What the proxy does for you automatically

You do not need to implement any of this, and you should not duplicate it.

**Disposition coercion** (`safe_disposition`). A non-`Ordinary` disposition is
forced back to `Ordinary` when any of these hold, with a `debug!` note:
- the request is an upgrade request or `CONNECT` (checked on the **union** of
  the downstream session and the possibly-rewritten upstream request);
- the downstream body is empty-and-done (prevents `Transfer-Encoding: chunked`
  + `0\r\n\r\n` on a bodyless request — a smuggling/desync primitive against a
  pooled upstream connection);
- the upstream request is below HTTP/1.1 (H1 pump only; no chunked framing).

The facts backing this are latched so a peer's `RST_STREAM` cannot retract them.
**Cost note:** the coercion short-circuits before collecting any facts when the
disposition is already `Ordinary`, so a request that does not use the feature
pays one inlined constant return plus an enum compare.

**Upstream framing per protocol.** You never need to know the selected upstream
protocol:

| | Ordinary | Bodyless | Streamed |
|---|---|---|---|
| H1 upstream headers | untouched | remove CL + TE | remove CL + TE, set `Transfer-Encoding: chunked`, **preserving any non-`chunked` transfer codings** |
| H2 upstream headers | untouched | remove CL + TE | remove CL + TE |
| H2 HEADERS END_STREAM | legacy `send_end_stream && body_empty` (declaration) | `send_end_stream` | always `false` |
| H2 empty-DATA EOS fallback | legacy | when headers did not carry EOS | when the body is already finished-and-empty |

CL and TE are never forwarded together. `Bodyless` honors an explicit
`set_send_end_stream(false)` (the gRPC-over-HTTP/2 MUST).

**Termination hygiene**, keyed by the **downstream** protocol (not the pump):
- H1 downstream: `set_keepalive(None)`, connection not reused (unread request
  bytes make it untrustworthy);
- H2 downstream: only the current stream is affected; the multiplexed
  connection stays usable;
- upstream: H1 discarded, H2 gets `send_reset(CANCEL)` only when the sibling
  future was dropped mid-flight;
- **no `Error` is produced** — terminate bypasses `error_while_proxy` retry
  classification and `fail_to_proxy` response generation entirely;
- cache write locks and miss handlers are released (`cache.disable`);
- custom-message readers/writers are restored.

**Retry suppression** at four points: the retry-buffer gate, the
`error_while_proxy` wrapper, `fail_to_connect`, and the retry loop. Plus an
unconditional guard: never retry once a final response is committed.

**Connection-reuse safety.** An H1 upstream connection whose request body was
not completed (chunked without its terminator, `Content-Length` short) is never
returned to the pool.

## 4. Limits and gotchas — read before designing against this

1. **`request_trailer_filter` fires only when the pump owns the body read.**
   If the application consumed the body before the pump started (without
   registering a replay buffer), the pump never observes the EOF and the hook
   does not fire. It is also skipped when the downstream read is abandoned as
   futile (see 3 below). An inspection policy that must see trailers **must not
   pre-read the body**.
2. **Trailers are detected but never forwarded.** The fact is transport-observed
   (H1: real fields in the chunked terminal section; H2: `RecvStream::trailers()`).
   A `Trailer:` declaration header alone does not count. If you need to reject
   trailer-bearing requests, do it in this hook.
3. **Futile-read termination.** When the upstream response is already complete
   and the downstream body is declared empty, the pump stops waiting for an
   end-of-stream that may never arrive, and synthesizes exactly one application
   EOS event. The transport never said the body ended.
4. **`Terminate` after the upstream request headers were written costs an H1
   upstream connection** (it cannot be pooled). If you can decide earlier,
   reject in `request_filter`.
5. **H1 truncation caveat.** Terminating with large unread request bytes in
   flight can produce an RST that races your local reply; a client that does not
   read concurrently may never see it. Same shape as
   `close_on_response_before_downstream_finish`.
6. **`request_retry_allowed == false` makes `Session::retry_buffer_truncated()`
   meaningless** — it reports `false` ("nothing truncated") because no buffer was
   ever allocated. If you override `error_while_proxy` and key on it, consult the
   predicate too.
7. **Custom-connector sessions**: a non-`Ordinary` disposition fails closed with
   a typed error; `request_trailer_filter` never fires (facts are `None`);
   `request_body_filter_action` does run, and `Terminate` there is a typed
   fail-closed error (that pump implements no terminate propagation).
8. **Serial latency is the accepted cost.** With per-chunk external processing,
   an upstream early response or a newly observed downstream disconnect can be
   delayed by the sum of the remaining processors' per-message timeouts for the
   current chunk. This was accepted in the design; it is not a defect.

## 5. Mapping to ExtProc `STREAMED`

| ExtProc need | Interface element |
|---|---|
| per-chunk async inspection/mutation | `request_body_filter_action` (hold bytes in `CTX`, blank the current chunk, emit the assembled result on a later chunk) |
| back-pressure | inherent: the pump does not read the next downstream chunk until the hook returns |
| mid-body rejection | write the local reply, then return `Terminate` |
| rewritten length | `Streamed` disposition |
| non-replayable request | `request_retry_allowed → false` (flip it when the first streamed candidate is selected, before the first upstream attempt) |
| trailer rejection | `request_trailer_filter → Terminate` |
| capability gating | facts returning `None`, plus the stock-build rejection |

Response-side streaming (SSE reassembly on the downlink) is **not** part of this
interface. It needs the AI-0a `response_body_stream_filter` work, which is
designed but unimplemented. This interface deliberately shaped its termination
contract so that work can reuse it.

## 6. Deltas for code that does not use the interface

Behavior changes on shared paths, all verified:

**Fixes you get for free.** H2 benign post-EOF reset classification in both
directions (a client that finishes its upload then cancels, or an upstream that
resets after a complete response, no longer produces a spurious error and no
longer destroys a healthy pooled connection); the response-committed retry guard
(closes a real body-concatenation corruption); the H1 unfinished-body pooling
refusal; `drain_request_body`'s restored short-circuit.

**Actual behavioral deltas.**
1. H2 `is_body_done()` is now the transport EOS fact (latched, monotonic), not
   an inference from `Content-Length: 0`. Upstream framing still forwards `CL: 0`
   as a promise, so completion timing for normal clients is unchanged.
2. An application using only the legacy `request_body_filter` now receives one
   `(None, end_of_stream = true)` event for a bodyless request routed to an
   **H2 upstream**, where it previously received zero. The two pumps are now
   symmetric.
3. `try_join!` → `biased select!`: request-body forwarding is deterministically
   polled before response forwarding within a wakeup. Outcomes identical.
4. Per-request cost: one extra `trailers()` poll at H2 request EOF (returns
   immediately when the stream is recv-closed), a few new bool fields. Per body
   event: one extra `Box::pin` **only** for applications still using the legacy
   hook — override `request_body_filter_action` directly and the count is
   unchanged from before.

## 7. Known residual risks

- The pre-existing `pingora-proxy` integration suites (`test_basic.rs`,
  `test_upstream.rs`) require a local openresty and **were never run** during
  this work. Run them before relying on the connection-pooling paths.
- No downstream request-body idle timeout: a client that stalls mid-upload can
  still pin a pump. Pre-existing, explicitly out of scope.
- Terminate-driven upstream `RST_STREAM` is not rate-limited (application-layer
  concern).
- Custom-connector pump: no end-to-end test coverage.
- ~~The `body-buffering` feature name is now misleading (the fork carries far
  more than body buffering). Renaming it to something like `pingora-fork` —
  with an alias — is cheaper before the Edgion port than after.~~
  **Resolved more thoroughly on 2026-08-03**: the flavor feature pair was
  removed entirely; the Gateway is fork-only with plain `pingora-*` deps.

## 8. Next steps on the Edgion side

In order. Do not skip 2.

1. **Update the fork family in `Cargo.lock`** — all six aliased crates
   (`pingora-{core,http,proxy,limits,load-balancing,ketama}-edgion`).
2. **Regression gate before any new code**: build and test Edgion on **both**
   flavors (`body-buffering` and `upstream-pingora`) against the new fork. The
   fork changed shared behavior (§6), so any breakage must be attributed here,
   not after new feature code is layered on.
   *(Executed 2026-08-03 as written — both flavors green. Immediately after,
   the flavor duality itself was removed (fork-only build), so future
   regression gates are single-flavor.)*
3. **Port the ExtProc `STREAMED` implementation.** The archived prototype is at
   `04-deferred-implementation/edgion.patch`. Port **behavior, not API names** —
   the prototype predates this interface and several semantics changed (fail-close
   on `Bodyless`, the coercions, the futile-read EOS, latched facts).
4. **Add the per-request gate**: compute a single `bool` in `CTX` at
   `request_filter` time (after the route unit and resolved plugin lists exist)
   so `request_body_filter_action` early-returns with one field test instead of
   walking plugin lists and cloning `Arc`s per chunk.
5. **Re-run an independent design review** of the Edgion side against this
   contract, then the full build matrix.
