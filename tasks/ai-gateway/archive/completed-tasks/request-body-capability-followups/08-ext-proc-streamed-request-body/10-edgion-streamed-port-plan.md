# ExtProc STREAMED Request Body — Edgion-Side Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable `processingMode.requestBodyMode: STREAMED` in the ExtProc plugin by
driving the Pingora fork's canonical request-body transport interface
(`09-pingora-interface-contract.md`).

**Architecture:** A new flavor-free per-request handler seam: request-stage plugins
install a `RequestBodyStreamHandler` into `EdgionHttpContext`; the `ProxyHttp`
hook `request_body_filter_action` drives installed handlers per chunk (then runs
the mirror tee), `upstream_request_body_disposition` returns `Streamed` while any
handler is installed, and `request_retry_allowed` flips false monotonically. The
ExtProc plugin opens one long-lived gRPC `Process` stream per request (headers
verdict + per-chunk body verdicts on the same stream).

**Tech Stack:** Rust, tokio, tonic; Pingora fork `b1d520ea` (already locked).
No new dependencies.

## Global Constraints

- Contract obligations (09 doc §2): finish the downstream response before
  returning `Terminate`; `request_retry_allowed` monotonic; never pre-read the
  body for a streamed policy.
- HTTP body invariants (AGENTS.md): no second request-body cache — the handler
  must hold at most the current chunk; per-chunk state only.
- Fail policy: ExtProc is non-auth → existing `failure_mode_allow` knob governs
  transport/verdict failures (fail-close default). No fifth failure shape.
- The prototype patch `04-deferred-implementation/edgion.patch` is lost; port
  behavior from the contract §5 mapping, not from code.
- `request_trailer_filter` is deliberately NOT implemented: the proxy never
  forwards request trailers (contract §4.2), so trailer-bearing requests lose
  nothing inspectable. Record this in the mod.rs comment.
- English-only source content; `cargo fmt` + `clippy` clean per task.

---

### Task 1: Schema unlock (edgion-resources)

**Files:**
- Modify: `edgion-resources/src/resources/edgion_plugins/plugin_configs/ext_proc.rs`
- Modify: `edgion-resources/src/resources/edgion_plugins/edgion_plugin.rs` (comment only)

**Interfaces:**
- Produces: `BodySendMode::is_streamed(&self) -> bool`; request-mode validation
  admits `Streamed`.

- [ ] **Step 1.1:** Flip the existing test `rejects_streamed_request_body` into
  `accepts_streamed_request_body` (asserts `validation_error().is_none()`); keep
  `rejects_buffered_response_body` and add `rejects_streamed_response_body`.
  Run: `cargo test -p edgion-resources ext_proc` → new test FAILS.
- [ ] **Step 1.2:** Implement: add `pub fn is_streamed(&self) -> bool` to
  `BodySendMode`; extend `is_supported_request()` to include `Streamed`; update
  the request-mode error string to "supports NONE, SKIP, BUFFERED, or STREAMED";
  update the `Streamed` variant doc comment ("Stream body chunks to the
  processor; requests only") and the module doc header.
- [ ] **Step 1.3:** In `edgion_plugin.rs` the `ExtProc(cfg)` BodyRequirement arm
  stays keyed on `is_buffered()`; add a comment: STREAMED intentionally has no
  BodyRequirement — it must not trigger capture (contract: never pre-read).
- [ ] **Step 1.4:** `cargo test -p edgion-resources` → PASS.

### Task 2: Handler seam (edgion-gateway runtime)

**Files:**
- Create: `edgion-gateway/src/plugins/runtime/traits/request_body_stream.rs`
- Modify: `edgion-gateway/src/plugins/runtime/traits/mod.rs`,
  `edgion-gateway/src/plugins/runtime/traits/session.rs`,
  `edgion-gateway/src/plugins/runtime/session_adapter.rs`,
  `edgion-gateway/src/ctx.rs`

**Interfaces (produced):**
```rust
pub enum ChunkVerdict {
    Continue,
    Terminate { status: u16, body: Option<String> },
}
#[async_trait]
pub trait RequestBodyStreamHandler: Send {
    fn name(&self) -> &str;
    /// Inspect/mutate the current chunk in place. `chunk` may be None with
    /// end_of_stream=true (pure EOS event).
    async fn on_chunk(&mut self, chunk: &mut Option<bytes::Bytes>, end_of_stream: bool) -> ChunkVerdict;
    /// Drain accumulated access-log entries (returns Some at most once at a time).
    fn take_log(&mut self) -> Option<PluginLog>;
    /// Dye plan captured at install time, re-installed by the hook for
    /// record_outcome during body-phase failures.
    fn dye_plan(&self) -> Option<Arc<CompiledDyePlan>>;
}
```
PluginSession additions (defaults keep mocks working):
```rust
/// Whether the transport can drive per-chunk streamed handlers.
fn request_body_stream_capable(&mut self) -> bool { false }
/// Install a handler; false = capability unavailable (caller must fail per policy).
fn install_request_body_stream_handler(&mut self, h: Box<dyn RequestBodyStreamHandler>) -> bool { false }
```
CTX addition: `pub request_body_stream: Vec<Box<dyn RequestBodyStreamHandler>>`
(default empty; NEVER cleared once populated — this emptiness test is both the
per-chunk fast-path gate and the monotonic retry predicate).

- [ ] **Step 2.1:** Write the trait file + ctx field + session methods.
  Adapter impl: `request_body_stream_capable` returns
  `self.inner.as_downstream_mut().request_headers_end_stream().is_some()`
  (facts `None` on Custom/Subrequest sessions → incapable, contract §1.3);
  `install_request_body_stream_handler` pushes into `self.ctx.request_body_stream`
  and returns true only when capable.
- [ ] **Step 2.2:** Unit tests in session_adapter.rs test module: real H1
  test_session → capable == true and install returns true + vec length 1;
  MockPluginSession default → install returns false.
- [ ] **Step 2.3:** `cargo test -p edgion-gateway request_body_stream` → PASS;
  `cargo check --workspace --all-targets` → PASS.

### Task 3: ProxyHttp hook wiring (proxy_http)

**Files:**
- Create: `edgion-gateway/src/routes/http/proxy_http/pg_request_body_filter_action.rs`
- Modify: `edgion-gateway/src/routes/http/proxy_http/mod.rs`,
  `edgion-gateway/src/routes/http/proxy_http/pg_logging.rs`

**Interfaces:**
- Consumes: `ctx.request_body_stream`, `ChunkVerdict`, existing mirror tee
  `pg_request_body_filter::request_body_filter`.
- Produces: trait impls `request_body_filter_action`,
  `upstream_request_body_disposition`, `request_retry_allowed` on `EdgionHttpProxy`.

- [ ] **Step 3.1:** Implement `pg_request_body_filter_action::request_body_filter_action`:
```rust
pub async fn request_body_filter_action(
    edgion_http: &EdgionHttpProxy, session: &mut Session,
    body: &mut Option<Bytes>, end_of_stream: bool, ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<RequestBodyAction> {
    if !ctx.request_body_stream.is_empty() {
        let mut handlers = std::mem::take(&mut ctx.request_body_stream);
        for handler in handlers.iter_mut() {
            match handler.on_chunk(body, end_of_stream).await {
                ChunkVerdict::Continue => {}
                ChunkVerdict::Terminate { status, body: reply } => {
                    ctx.current_dye_plan = handler.dye_plan();
                    write_terminate_reply(session, ctx, status, reply).await;
                    drain_handler_logs(&mut handlers, ctx);
                    ctx.request_body_stream = handlers;
                    return Ok(RequestBodyAction::Terminate);
                }
            }
        }
        if end_of_stream {
            drain_handler_logs(&mut handlers, ctx);
        }
        ctx.request_body_stream = handlers;
    }
    // Mirror tee sees the post-mutation chunk (what the upstream receives).
    pg_request_body_filter::request_body_filter(edgion_http, session, body, end_of_stream, ctx).await?;
    Ok(RequestBodyAction::Continue)
}
```
  `write_terminate_reply` builds a `PingoraSessionAdapter` and uses its
  gRPC-aware `write_response_header` + `write_response_body` (the ErrResponse
  path plumbing), then sets `ctx.local_reply_written = true`.
  `drain_handler_logs` pushes `StageLogs { stage: "request_body_filters", filters, edgion_plugins: vec![] }`.
- [ ] **Step 3.2:** In mod.rs: replace the `request_body_filter` trait impl with
  `request_body_filter_action` (the legacy hook is never called once the action
  is overridden; the tee helper file stays); add
  `upstream_request_body_disposition` → `Streamed` iff `!ctx.request_body_stream.is_empty()`
  else `Ordinary`; add `request_retry_allowed` → `ctx.request_body_stream.is_empty()`.
  Comment block: why `request_trailer_filter` stays default (trailers never
  forwarded proxy-wide) and why the vec is never cleared (monotonicity).
- [ ] **Step 3.3:** In `pg_logging.rs`: before assembling stage logs, drain any
  leftover handler logs (client abort / early upstream response path).
- [ ] **Step 3.4:** Audit `pg_error_while_proxy.rs` / `pg_fail_to_connect.rs`
  for `retry_buffer_truncated()` reliance (contract §4.6); where found, also
  consult `ctx.request_body_stream.is_empty()`. Record findings in the commit
  message even if no change is needed.
- [ ] **Step 3.5:** `cargo check --workspace --all-targets` + existing mirror
  tests still green: `cargo test -p edgion-gateway mirror` → PASS.

### Task 4: ExtProc long-lived stream (ext_proc/client.rs)

**Interfaces (produced):**
```rust
pub struct ExtProcStream { /* tx, tonic Streaming, per-message timeout */ }
impl ExtProcClient {
    /// Open one Process stream for a whole-request transaction.
    pub async fn open_stream(&self) -> Result<ExtProcStream, tonic::Status>;
}
impl ExtProcStream {
    /// Send one message and await its phase-matched response under the
    /// per-message timeout. ImmediateResponse is always accepted.
    pub async fn exchange(&mut self, req: ProcessingRequest, phase: ProcessingPhase)
        -> Result<ProcessingResponse, tonic::Status>;
}
```

- [ ] **Step 4.1:** Implement `open_stream` (channel(4) + `ReceiverStream` into
  `client.process`, initial call bounded by the configured timeout) and
  `exchange` (send → `tokio::time::timeout(self.timeout, responses.next())`;
  reuse `ProcessingPhase::accepts` for phase matching; wrong phase →
  `failed_precondition`, timeout → `deadline_exceeded`, stream end → `aborted`).
  Make `ProcessingPhase` and its `accepts` visible to the plugin module.
- [ ] **Step 4.2:** Unit tests with an in-process tonic server (follow the
  existing client.rs test patterns if present; otherwise test phase matching
  and timeout against a local fake `Process` service bound to a random port).
- [ ] **Step 4.3:** `cargo test -p edgion-gateway ext_proc` → PASS.

### Task 5: ExtProc streamed mode (ext_proc/plugin.rs + streamed.rs)

**Files:**
- Create: `edgion-gateway/src/plugins/http/ext_proc/streamed.rs`
- Modify: `edgion-gateway/src/plugins/http/ext_proc/plugin.rs`, `mod.rs`

**Interfaces:**
- Consumes: Task 2 session methods, Task 4 `ExtProcStream`.
- Produces: `ExtProcStreamedBodyHandler` implementing `RequestBodyStreamHandler`.

- [ ] **Step 5.1 (tests first):** In streamed.rs, define
  `trait BodyExchange: Send { async fn exchange_body(&mut self, HttpBody) -> Result<ProcessingResponse, tonic::Status>; }`
  implemented by `ExtProcStream`; the handler holds `Box<dyn BodyExchange>`.
  Write failing unit tests with a mock exchange:
  - body mutation replaces the current chunk; ClearBody empties it;
  - `ImmediateResponse` → `Terminate { status, body }`;
  - transport error, `failure_mode_allow=false` → `Terminate { 500, "ext_proc unavailable" }`;
  - transport error, `failure_mode_allow=true` → `Continue` and handler
    deactivates (`done`), later chunks skipped without exchange;
  - wrong-phase verdict follows the failure policy;
  - EOS event with `None` chunk sends `HttpBody { body: vec![], end_of_stream: true }`.
- [ ] **Step 5.2:** Implement the handler until tests pass. Log entries via an
  owned `PluginLog` ("ext_proc body chunk"/failure lines, same `la!` style);
  header mutations inside a body verdict are ignored with a log note (upstream
  request headers are already committed).
- [ ] **Step 5.3:** Wire `run_request` streamed branch:
  - `is_streamed()` && request stage; else existing behavior untouched.
  - `session.declares_body() == Some(false)` → header-only degenerate path
    (headers `end_of_stream: true`, no handler, no retry/disposition impact).
  - `!session.request_body_stream_capable()` → `fail_mode(session, log, "streamed unsupported", None)`.
  - `open_stream()` error → fail_mode; headers SEND → `exchange(RequestHeaders { end_of_stream: false })`,
    apply verdict exactly like the buffered path except `ContinueAndReplace`
    at the header phase → `fail_mode "bad verdict"` (no cache to replace);
    `ImmediateResponse` → `immediate_reply`.
  - Install `ExtProcStreamedBodyHandler { stream, failure_mode_allow, log, dye_plan: ctx dye plan clone, done: false }`;
    install returning false → fail_mode (defense-in-depth after 5.3's capability check).
- [ ] **Step 5.4:** `cargo test -p edgion-gateway ext_proc` + full
  `cargo test --all` → PASS.

### Task 6: Integration scenario

**Files:**
- Modify: `edgion-tests/src/server/test_server.rs` (fake processor: streamed body support)
- Create: `edgion-tests/integration/conf/EdgionPlugins/ExtProc/07_EdgionPlugins_ext-proc-streamed.yaml`
  (+ HTTPRoute yaml, following files 01-06 naming)
- Modify: `edgion-tests/src/client/suites/edgion_plugins/ext_proc/ext_proc.rs`

- [ ] **Step 6.1:** Extend the fake processor: on `RequestBody` messages reply
  per chunk (configurable: mutate payload / immediate response after N bytes).
- [ ] **Step 6.2:** Scenario A: POST with body through a streamed ExtProc route;
  assert the upstream echo shows the mutated body and the response succeeds.
  Scenario B: processor returns ImmediateResponse mid-body; assert the client
  receives that status and the access log carries the ext_proc entry.
- [ ] **Step 6.3:** Register the suite item in the ExtProc sweep (keep suite
  counts consistent with `run_integration.sh` guards). Run:
  `./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r EdgionPlugins -i ExtProc` → PASS.

### Task 7: Docs + guards

- [ ] **Step 7.1:** Update ExtProc feature/skill docs (grep `skills/` +
  `docs/en` + `docs/zh-CN` for `requestBodyMode`/ExtProc BUFFERED wording) to
  state STREAMED is supported for requests; response bodies remain streaming-only.
  AGENTS.md invariant #2's "ExtProc response BUFFERED rejected" stays true.
- [ ] **Step 7.2:** Full gate: `cargo fmt --all`,
  `cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets`,
  `python3 cicd/checks/validate_agent_docs.py`, `cargo test --all`.

## Explicit non-goals (this iteration)

- Response-side streaming (AI-0a `response_body_stream_filter`) — unimplemented
  upstream; out of scope.
- `BUFFERED_PARTIAL`; request trailer forwarding/rejection; per-request overall
  deadline beyond the per-message timeout (contract §4.8 accepts serial latency).
- `Bodyless` disposition (no consumer yet).
