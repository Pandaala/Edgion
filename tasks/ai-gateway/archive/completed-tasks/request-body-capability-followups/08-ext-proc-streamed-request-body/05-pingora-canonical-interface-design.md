# Canonical Pingora Request-Body Transport Interface — Design

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-02 |
| Status | approved in collaborative design session; ready for implementation planning |
| Scope | Pingora fork only (`edgion` branch, baseline `b829a34`). The Edgion-side rebase of `04-deferred-implementation/edgion.patch` is a separate follow-up task. |
| Satisfies | Re-evaluation step 1 of `04-deferred-implementation/README.md`; unblocks decision D21 |
| Inputs | `04-deferred-implementation/pingora.patch` (validated prototype, ported by behavior, not by name), `04-deferred-implementation/pingora-design/01-body-stream-design.md` (AI-0a v4, historical input) |

## 1. Goal and scope

Define and land the canonical request-body transport interface in the Pingora
fork that task 8 (ExtProc streamed request body) requires: a typed request-body
terminal action, transport-observed request facts (headers-EOS, trailer
presence), an application-selected upstream body framing disposition, and a
canonical retry-eligibility predicate.

The interface is designed as a first-class fork API, not a feature-local seam:
behavior is defined for all four downstream session types, defaults preserve
today's behavior exactly, and the termination contract is shaped so that the
future response-side BodyStream `Finish` (AI-0a) can reuse the same channel.

**Unification principle: unify the contract, not the mechanism.** The request
side (this design) and the future response side share three things — a typed
half-outcome instead of a flag hack or a fabricated `Error`, one connection
hygiene table, and the "no `Error` produced" invariant. The request side uses
`select!`-driven sibling cancellation because immediate cancellation is a
blocking requirement of the accepted task-8 design; the response side may keep
its flag-plus-synthesized-`Done` mechanism (AI-0a's choice) while plugging into
the same contract.

Out of scope: any Edgion change, response-side hooks, terminate support in the
custom-connector pump, request-trailer forwarding, upstreaming to
cloudflare/pingora.

### Acceptance standard

Fork-self-contained verification: full existing test suite green
(`pingora-core --lib`, `pingora-proxy --lib`, integration tests), fmt/clippy
clean, plus the new coverage in Section 7, merged to the `edgion` branch. No
temporary path override into Edgion; Edgion integration verification belongs to
the follow-up task.

## 2. Public API surface

Two public types and four `ProxyHttp` methods (all with defaults; none of the
existing 18 Edgion-implemented methods change signature), plus two `Session`
fact queries.

### 2.1 Types (`pingora-proxy`, re-exported via `prelude`)

```rust
/// The action selected by an application request-body hook.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestBodyAction {
    /// Continue proxying the current request-body event.
    Continue,
    /// Stop proxying this request after the application completed or aborted
    /// the downstream response.
    Terminate,
}

/// How the proxy should frame the request body sent to an upstream.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UpstreamRequestBodyDisposition {
    /// Preserve Pingora's ordinary request-body framing behavior.
    #[default]
    Ordinary,
    /// The application guarantees that no upstream request body will follow.
    Bodyless,
    /// The body is streamed and its final length is not known when headers
    /// are sent.
    Streamed,
}
```

Naming decisions carried over from the prototype, with rationale:

- `request_body_filter_action` keeps the `_action` suffix to make the
  "extended variant of `request_body_filter`" relationship explicit.
- `RequestBodyAction` is deliberately not direction-neutral: the future
  response side has its own richer action enum (`PassThrough`/`Emitted`/
  `Finish`). What the two directions share is the termination channel and the
  hygiene rules (Section 3), not the action enums.

### 2.2 `ProxyHttp` trait methods

| Method | Sync/async | Default behavior |
|---|---|---|
| `request_body_filter_action(session, &mut Option<Bytes> body, end_of_stream, ctx) -> Result<RequestBodyAction>` | async | Invokes the existing `request_body_filter`, returns `Continue` — source-compatible; unmodified applications change nothing |
| `request_trailer_filter(session, ctx) -> Result<RequestBodyAction>` | async | `Continue`. Called after the trailer-presence fact is established and before any synthetic body EOS could be delivered. Pingora neither exposes nor forwards the trailer fields themselves |
| `upstream_request_body_disposition(session, ctx) -> UpstreamRequestBodyDisposition` | sync | `Ordinary`. Queried once per upstream attempt, before upstream request headers are written |
| `request_retry_allowed(session, ctx) -> bool` | sync | `true`. Returning `false` also suppresses native retry buffering |

Sync/async split rationale: `ProxyHttp` is an `#[async_trait]`, so async
methods cost one `Box::pin` per invocation, while sync default methods
monomorphize and inline away (`SV` is a generic parameter). The disposition and
retry predicates are pure once-per-request(-attempt) queries and therefore
sync; the disabled path pays nothing measurable.

### 2.3 `Session` fact queries (`pingora-core`)

| Method | Semantics | H1 source | H2 source | Custom/Subrequest |
|---|---|---|---|---|
| `request_headers_end_stream() -> Option<bool>` | whether the downstream transport ended the request at its header section | H1: snapshotted once at body-reader initialization, stable thereafter (a chunked body — even an empty one — is `false`; `Content-Length: 0` is `true`, because H1 framing is declarative — a deliberate, protocol-inherent asymmetry with H2) | END_STREAM flag of the HEADERS frame, snapshotted at session construction | `None` |
| `request_trailers_present() -> Option<bool>` | whether the request carried **actual** trailer fields (meaningful after body EOF; a `Trailer` declaration header alone does not count) | recorded while parsing the chunked terminal section | result of `RecvStream::trailers()` after DATA ends | `None` |

`None` is a capability declaration: this session type does not provide the
fact. An application that requires the fact must fail closed (Edgion: reject
STREAMED plugin construction when the facts are unavailable, the same policy
as its stock-build rejection).

## 3. Termination propagation and connection hygiene

### 3.1 Shared outcome type

The prototype defined identical private enums in `proxy_h1.rs` and
`proxy_h2.rs`; the canonical interface defines it once:

```rust
// proxy_common — crate-internal, not public API
pub(crate) enum DownstreamRequestOutcome {
    /// Normal completion; the bool is downstream connection reusability.
    Complete(bool),
    /// Application termination: proxying of this request stops here.
    Terminate,
}
```

### 3.2 Join-structure change

In H1 `proxy_1to1` and the H2 equivalent (two isomorphic sites),
`tokio::try_join!` is replaced by pinning both half futures and driving them
with `tokio::select!`:

- **Downstream half returns `Terminate` first** → drop the sibling upstream
  future. This is the immediate-cancellation step: the upstream half parked in
  `read_response_task()` / `tx.send` is cancelled synchronously at its await
  point — no `read_timeout` wait, no detached task.
- **Downstream half returns `Complete` first** → `await` the upstream half
  (equivalent to the original `try_join!` semantics).
- **Upstream half returns first** → on `Ok`, continue awaiting the downstream
  half; on `Err`, fail the exchange (as today).

`Terminate` originates inside `send_body_to_pipe` (H1) / `send_body_to2` (H2)
when `request_body_filter_action` or `request_trailer_filter` returns
`Terminate`, and propagates outward through typed returns.

**Application contract:** before returning `Terminate`, the application has
already finished the downstream response itself — either it wrote a local
reply, or it observed an already-committed response and chose to abandon.
Pingora never writes a response on the terminate path.

### 3.3 Connection hygiene table

| | Downstream connection | Upstream connection | Error path |
|---|---|---|---|
| H1 | `set_keepalive(None)`, not reusable — unread request bytes make the connection untrustworthy | discarded, not pooled (`upstream_reuse = false`) | returns `(false, false, None)`: **no `Error` produced** |
| H2 | shared multiplexed connection remains usable — only this stream is affected and the application already finished this stream's response | `client_body.send_reset(CANCEL)` resets only the current upstream request stream | returns `(false, None)`: **no `Error` produced** |

**Hygiene is keyed by the downstream session type, not by the pump**
[corrected during implementation — final-review finding C1]. The pumps are
selected by *upstream* protocol, so an H1 downstream can terminate inside
the H2 pump. Both pumps therefore apply the uniform downstream-safe form:
`set_keepalive(None)` (a no-op on an H2 downstream) and a `false` reuse
return. For H2 downstreams the `false` merely skips the session-level
finish/reuse call, which never touches the shared multiplexed connection —
other streams stay usable (test-proven); for H1 it prevents an unbounded
`drain_request_body()` of a request body the application just refused.

"No `Error` produced" is the core invariant of this channel: `Terminate`
never reaches `error_while_proxy` (no retry classification) and never reaches
`fail_to_proxy` (no second response). This satisfies task-8 design §5.1
("typed terminal outcome; cancellation never reaches generic error
handling").

### 3.4 Contract reserved for the response side

When AI-0a's `Finish` lands, it reuses exactly three things: (1) a half
returns a typed outcome rather than a flag hack or fabricated `Error`; (2) the
rows of the same connection hygiene table; (3) the no-`Error` invariant. Its
trigger direction differs (the downstream half terminates the response stream
rather than the request stream) and its mechanism details (synthesized
`HttpTask::Done`) stay in AI-0a's own design.

### 3.5 Known semantic boundary (documented, not solved)

Inherited from task-8 design §5.2: in the current pump structure, a queued
upstream-response event is not serviced while the body hook is awaited, so the
"upstream early response vs. application local reply" race is decided by the
application checking `response_written()` before writing its local reply.
Returning `Terminate` after a response is already committed is legal and means
"abort the remaining streams; write no second response."

## 4. Transport facts and framing

### 4.1 Trailer detection

Both detections are transport observations, never inferred from declaration
headers:

- **H1**: the chunked terminal-section parser
  (`TrailersEndParseState::Complete`) gains a bool — whether actual trailer
  fields were read (`trailers_read > 0`). `BodyReader` records the flag;
  `reinit()` clears it so the next request on a keepalive connection is not
  polluted.
- **H2**: after `read_body_bytes()` observes the end of DATA, it awaits
  `RecvStream::trailers()` and records `is_some_and(|f| !f.is_empty())`.
  Ordering guarantee: the trailer fact is established before the early-body
  buffer's `finish_capture`, so the pump always knows trailer presence before
  it could emit a synthetic EOS — which is what makes the
  `request_trailer_filter` call timing (at EOF, before EOS delivery) sound.

### 4.2 Headers-EOS

- **H2**: snapshot `request_body_reader.is_end_stream()` at session
  construction — i.e. whether the HEADERS frame carried END_STREAM;
  subsequent reads cannot change it.
- **H1**: `is_body_empty()` is itself the transport fact (no CL/TE means no
  body) and is reused directly.

### 4.3 Accompanying H2 semantic correction (deliberate behavior change)

H2 `is_body_done()` is tightened from `is_body_empty() || is_end_stream()` to
`is_end_stream()` only. Basis: task-8 design §3.1 — an H2 request with
`Content-Length: 0` but no END_STREAM **is not bodyless**; the old logic
treated it as finished by inferring from CL instead of the transport fact.
This correction affects ordinary traffic too; the prototype already adjusted
the two affected existing tests. The byte-neutrality verification treats this
as the single known, intentional behavioral difference.

### 4.4 Disposition application points

Each pump applies its own protocol rules, so the application never needs to
know the selected upstream protocol — this is how the task-8 §6 requirement
("the framing synchronizer must know the selected upstream protocol") is met.

| Application point | Ordinary | Bodyless | Streamed |
|---|---|---|---|
| H1 upstream request headers | untouched | remove CL + TE | remove CL + TE, then explicitly set `Transfer-Encoding: chunked` (an H1 request with neither header means a zero-length body, so removal alone is a correctness bug) |
| H2 upstream request headers | untouched | remove CL + TE | remove CL + TE (H2 relies on DATA END_STREAM) |
| H2 HEADERS END_STREAM decision | legacy `send_end_stream && body_empty` | always `true` | always `false` |

One additional H2 detail: the existing "send END_STREAM on an empty DATA frame
when the body is empty" path is kept only under `Ordinary` — `Bodyless` has
already sent EOS on HEADERS, and `Streamed` must not send an early EOS.

Query timing: once per upstream attempt, before upstream request headers are
written and before `begin_request_body_replay()`. By construction, CL and TE
are never forwarded together.

## 5. Retry suppression

### 5.1 The four consumption points of `request_retry_allowed`

One predicate spans every retry decision — the fork-side landing of task-8
§9's "canonical predicate":

1. **Retry-buffer gate** (H1/H2 pump startup): when `false`,
   `enable_retry_buffering()` is never called — the buffer is never enabled,
   so reused-connection replay and H2 predecided replay have no material.
   Task-8 §9 ("the fork does not enable or populate its retry buffer for such
   a request") is satisfied here.
2. **Mid-proxy errors** (the `error_while_proxy` wrapper in `lib.rs`): after
   the application's `error_while_proxy` returns, force `e.retry = false` when
   the predicate is `false` — the `Error` object itself stays honest for
   logging and external inspection.
3. **Connect failures** (the `fail_to_connect` path): same forcing. Covers
   task-8 §9's connection-establishment retry prohibition (backendRef plugins
   run per attempt; a reconnect could change the effective plugin chain).
4. **Retry-loop decision** (`lib.rs` main loop):
   `error.retry() && request_retry_allowed(...)` — the final gate, catching
   any `Error` source not wrapped by points 2/3.

### 5.2 Timing contract

The predicate is queried live at each decision point and never cached. This
lets an application flip from `true` to `false` mid-request (Edgion's case:
the flip happens when a streamed ExtProc candidate is selected, before the
first upstream attempt). The contract requires the application to keep the
predicate monotonic (once `false`, never `true` again); Pingora does not
verify this.

### 5.3 Separate commit: the response-committed retry guard

A general correctness fix, independent of this feature, and an invariant both
directions depend on:

```rust
// In the retry loop, before error.retry() is read.
if session.response_written().is_some_and(|r| {
    !r.status.is_informational() || r.status == StatusCode::SWITCHING_PROTOCOLS
}) {
    error.retry = false.into();
}
```

Semantics: once a final response is committed downstream, never retry — a
retry would concatenate two upstream responses into one downstream body. Two
status details: H1's `response_written()` may hold a 1xx informational
response (not final, does not block retry), while 101 is a final response
despite being 1xx (blocks). H2 can never store a 1xx, so the guard is
trivially safe there.

**[Corrected during implementation.]** AI-0a §3.7 claimed this guard closes
a pre-existing live hole (`H2Error` with `RetryType::ReusedOnly` retrying
after a committed response header). Runtime evidence during implementation
refuted that claim: mid-body errors are constructed non-retryable
(`Decided(false)`) on both protocols, and the only retryable
classifications exist on response-header reads, which are pre-commit by
definition — so no currently reachable path violates the invariant. The
guard is therefore a structural invariant pin against future drift in
error classification (e.g. after an upstream merge), not a fix for a live
defect. The accompanying end-to-end invariant test is
`h2_error_no_retry_after_header_sent_on_reused_conn`.

## 6. Custom/Subrequest boundary and disabled-path invariance

### 6.1 Custom connector

The prototype left `proxy_custom.rs` untouched, which leaves a silent
divergence trap: the custom pump calls `request_body_filter` directly, so an
application that only overrides `request_body_filter_action` would silently
run no filter on custom sessions. The canonical interface switches the custom
pump to the action hook as well: `Continue` proceeds as today; `Terminate`
returns a typed `Err` (explicit fail-closed — the custom pump has its own
independent join structure, and implementing terminate propagation there is
out of scope). `request_trailer_filter` never fires on custom sessions (the
fact is `None`).

### 6.2 Subrequest

Terminate propagation itself is session-type-agnostic (it lives in the pump
join structures keyed by upstream protocol); the fact queries returning `None`
are the only type-dependent surface. Connection hygiene actions degenerate to
no-ops on a subrequest downstream. Applications are expected not to activate
streaming plugins on subrequests; the interface adds no special branch for
them.

### 6.3 Disabled-path invariance (all hooks left at their defaults)

| Item | Behavior |
|---|---|
| Trait defaults | the action hook delegates to `request_body_filter` then returns `Continue`; everything else is a constant — existing applications change nothing and observe no semantic difference |
| `select!` replacing `try_join!` | with no `Terminate` ever produced, semantics are equivalent (concurrent polling, first-`Err` wins, both-`Ok` joined) |
| New per-request cost | two inlinable sync queries (disposition, retry predicate) + one bool in the H1 chunked terminal section |
| Per-body-event cost | on the default path, `Box::pin` count goes from 1 to 2 (the action wrapper plus the delegated `request_body_filter`); applications overriding the action hook directly stay at 1. Recorded honestly as a known minor cost |
| New `trailers().await` at H2 EOF | returns `None` immediately when the peer sent EOS on DATA; observe-only, no forwarding, wire behavior unchanged |
| **Deliberate behavior changes** | Section 4.3's H2 `is_body_done()` tightening (CL inference replaced by the transport fact), plus its implementation companions: early-EOS gates in both pumps (`is_body_empty() && is_body_done()`) so a `CL: 0`-without-END_STREAM H2 request yields exactly one upstream EOS and one app EOS event, and a restored short-circuit in H2 `drain_request_body()` (`is_body_done() \|\| is_body_empty()`) preserving its pre-tightening bound |

Implementation notes recorded during the deep review: the two public enums are
deliberately exhaustive (the future response side gets its own action enum;
Edgion is the sole consumer); `RequestBodyAction::Terminate` on H1 carries a
documented truncation caveat (large unread request bytes can cause an RST
that races the local reply on clients that do not read concurrently — same
shape as `close_on_response_before_downstream_finish`); non-`Ordinary`
dispositions are coerced to `Ordinary` with a warning on upgrade/CONNECT
requests and fail closed on custom-connector sessions.

## 7. Test plan and commit split

Five commits, each independently valid with the full suite green — also the
natural boundaries for optionally offering pieces upstream later:

| # | Commit | Content | Key tests |
|---|---|---|---|
| 1 | Response-committed retry guard | Section 5.3, independent of the rest | `retry_guard_ignores_1xx_but_not_101`, `h2_error_no_retry_after_header_sent_on_reused_conn` |
| 2 | Transport facts | `pingora-core`: H1/H2 trailer detection, headers-EOS, H2 `is_body_done()` tightening | chunked parse flag with/without trailers; end-to-end H2 trailer observation; the two corrected existing tests |
| 3 | Disposition/framing seam | `UpstreamRequestBodyDisposition` + the three application points | disposition × protocol framing unit tests; end-to-end four combinations (H1→H1 chunked, H1→H2 no TE, H2→H1 chunked, H2→H2 END_STREAM) + stale client CL removal |
| 4 | Termination channel | `proxy_common` outcome enum, `select!` restructure, `request_body_filter_action` / `request_trailer_filter`, custom fail-closed, connection hygiene | see below |
| 5 | `request_retry_allowed` | the four consumption points | retry buffer never enabled; connect failure not retried; status retry suppressed; `max_retries > 1` still yields exactly one upstream attempt |

Commit 4 integration coverage (the termination channel is the risk center):

- **Promptness**: with an upstream that never responds, `Terminate` completes
  immediately without waiting for `read_timeout` — directly validating the
  `select!` sibling-drop structural choice.
- **H1 hygiene**: after a mid-body terminate, the connection is not reused and
  a connection with unread bytes cannot parse a pipelined next request.
- **H2 hygiene**: only the current upstream stream receives RST(CANCEL);
  unrelated streams on the downstream connection remain usable.
- **No-`Error` invariant**: terminate triggers no `fail_to_proxy` (no second
  response, no generic 502); with retries configured, exactly one upstream
  attempt occurs.
- **Trailer path**: a trailer-bearing request fires `request_trailer_filter`;
  `Terminate` produces the application's response; with `Continue`, trailers
  are still not forwarded (current behavior, documented).
- **Compatibility**: `action_hook_defaults_to_legacy_body_filter_and_continue`
  (an application overriding only the legacy hook behaves identically) and a
  `request_future_is_send` compile-time assertion (the `select!` + pin
  restructure must not break `Send`).
- **Custom**: `Terminate` yields the typed fail-closed `Err`.

Disabled-path invariance is evidenced by the full existing suite passing, with
Section 4.3's tightening as the single known intentional difference.

## 8. Relationship to the archived prototype

`04-deferred-implementation/pingora.patch` is the feasibility evidence for
every mechanism above and the starting point for porting behavior. Per the
archive README, behavior is ported rather than names blindly preserved; the
canonical deltas over the prototype are:

1. the shared `DownstreamRequestOutcome` in `proxy_common` (the prototype
   duplicated it per file);
2. the custom-pump action-hook wiring with fail-closed `Terminate`
   (the prototype left the custom pump on the legacy hook);
3. the response-committed retry guard as its own commit (from AI-0a §3.7,
   absent from the prototype);
4. the five-commit structure with per-commit green suites (the prototype was
   one combined seam).
