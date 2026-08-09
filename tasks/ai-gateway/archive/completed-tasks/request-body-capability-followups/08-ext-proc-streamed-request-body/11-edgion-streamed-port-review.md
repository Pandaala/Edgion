# Edgion STREAMED Port — Independent Design Review Record

> Two review rounds are recorded here: Round 1 (single adversarial reviewer,
> same day as implementation) and Round 2 (three parallel deep reviewers over
> the full uncommitted diff, later the same day). All findings from both
> rounds were fixed before commit; see the Round 2 section at the end.

> Contract step 5 (`09-pingora-interface-contract.md` §8.5): an independent,
> adversarial review of the Edgion-side implementation against the consumer
> contract, run 2026-08-03 after the initial implementation and integration
> pass were green. All findings were fixed the same day; this file records
> what was found and how it was resolved, for future re-review context.

## Findings and resolutions

### 1. critical / CONFIRMED — cancellation-unsafe `mem::take` voided retry monotonicity

The hook `request_body_filter_action` takes the handler vector out of the ctx
across `.await` points. The proxy's `biased select!` drops the downstream
future mid-hook on an upstream error (fork `proxy_h2.rs` documents this), so
the taken handlers could be destroyed, leaving `ctx.request_body_stream`
permanently empty. Every predicate keyed on vector emptiness then flipped:
`request_retry_allowed` → true, disposition → `Ordinary`, both `buffer_ok`
retry gates → pass — re-arming retries that would replay nothing and forward
the remaining body uninspected under the original `Content-Length`.

**Fix:** monotonic latch `EdgionHttpContext::request_body_stream_active`, set
by `install_request_body_stream_handler`, never cleared. All four predicates
(`request_retry_allowed`, `upstream_request_body_disposition`, both
`buffer_ok` sites) now key on the latch. A dropped-future handler loss now
fails the request (retry suppressed) instead of bypassing inspection. The
vector remains the driving structure only.

### 2. major / CONFIRMED — terminate reply could splice into a committed response

`write_terminate_reply` did not check whether an upstream response was already
committed downstream. The fork treats a duplicate header write as a warn +
`Ok(())` no-op, so the follow-up body write would inject the terminate body
into a committed, unfinished upstream response (upload racing an early
chunked 200).

**Fix:** `session.response_written().is_some()` → observe and abandon (write
nothing, do not set `local_reply_written`, still return `Terminate`), per the
contract's "observe an already-committed response and abandon" obligation.

### 3. major / PLAUSIBLE — eager stream open could deadlock against grpc-go processors

`open_stream` awaited tonic's `process()` (response headers) before any
message was sent. grpc-go-style processors defer response headers until their
first `Recv`/`Send`, producing a mutual wait until the per-message timeout on
every request. The in-repo tonic echo server masked this.

**Fix:** lazy open — `open_stream` is now synchronous (builds the channel and
parks the un-awaited call future); the first `exchange` sends its message and
then resolves the open within the same per-message timeout
(`InboundState::Opening → Open`). Also saves one wait-RTT.

### 4. minor / PLAUSIBLE — bodyless degenerate path keyed on CL inference

`declares_body() == Some(false)` derives from `Content-Length`, which an H2
client can contradict (declare `CL: 0`, still send DATA) to skip inspection.

**Fix:** the streamed branch now gates on the latched transport fact via the
new `PluginSession::request_headers_end_stream()` (`Some(true)` → header-only
degenerate path, `Some(false)` → install handler, `None` → capability absent,
fail mode). This replaced the separate `request_body_stream_capable` method.

### 5. note / CONFIRMED — fail-open deactivation pinned the gRPC stream

**Fix:** `ExtProcStreamedBodyHandler.exchange` is now `Option`; fail-open
deactivation drops it, half-closing the processor stream immediately.

### 6. note / CONFIRMED — pre-decided retries rely on the proxy backstop

h2 GOAWAY/REFUSED_STREAM errors arrive `Decided(true)` and bypass Edgion's
`buffer_ok` gates; only the proxy's `request_retry_allowed` sampling stops
them. Sound by design — but it makes finding 1's latch load-bearing for these
paths. No code change; recorded as the reason the latch must stay monotonic.

## Verified-clean checklist (no change needed)

- Terminate obligations for ExtProc (`sanitize_status` clamps to 200–599, the
  reply is always awaited before `Terminate`).
- No pre-read: streamed mode maps to no `BodyRequirement`, no body-gate wrap,
  no `BodyStorage` capture.
- Buffered+streamed coexistence: capture-replay chunks traverse the hook, the
  `Streamed` disposition strips the CL that `sync_body_framing_headers`
  rewrites (fork applies disposition after `upstream_request_filter`).
- SKIP-headers streamed mode (body chunk as first message) is protocol-legal.
- Unknown-body EOS semantics match the fork's single-EOS guarantee.
- No concurrent ctx aliasing (single downstream future owns `&mut ctx`).
- Controller-side validation reuses the shared `edgion-resources`
  `validation_error`; no CRD/stage-enum drift.

## Round 2 (three parallel deep reviewers, 2026-08-03)

Scopes: (A) flavor-removal refactor, (B) streamed data-plane vs contract +
fork source, (C) ExtProc plugin/client/protocol + test adequacy. Round 1's
five fixes were all re-verified correct under adversarial tracing, with one
exception (B-C1 below refined fix 2). Reviewer C also conclusively disproved
a suspected critical (tonic `Endpoint::timeout` bounds time-to-response-
headers only — it does NOT kill long-lived `Process` streams mid-request).

### Fixed from Round 2

- **B-C1 (critical)**: the terminate-reply abandon check treated a relayed
  interim 1xx (`100 Continue`, `103`) as a committed response, losing the
  mandated rejection reply on ordinary `Expect: 100-continue` uploads. Fix:
  abandon only on a committed FINAL response (informational statuses except
  101 do not count).
- **B-I2 (important)**: on gRPC routes a terminate with status 200 wrote a
  bare HTTP reply with no `grpc-status`. Fix: `write_terminate_reply` now
  mirrors the request-stage `ErrResponse` choke point — every gRPC-route
  terminate goes through `send_grpc_error_direct` with the standard
  HTTP→gRPC status mapping.
- **B-I1 (important)**: the proxy coerces a `Streamed` disposition back to
  `Ordinary` for upgrade/CONNECT and pre-HTTP/1.1 requests while installed
  handlers would keep mutating chunks (length desync under preserved CL;
  post-101 tunnel bytes reaching the processor). Fix: the adapter install
  gate refuses those request shapes up front, so the plugin applies its
  failure policy instead of silently degrading.
- **C-1 (important)**: the bodyless degenerate path silently ignored a
  headers-phase `CONTINUE_AND_REPLACE`; now rejected as bad verdict, matching
  the non-degenerate streamed branch and the shipped docs.
- **C-2 (important)**: mid-body `ImmediateResponse` header mutations were
  silently dropped. Fix: `ChunkVerdict::Terminate` now carries
  protected-filtered header pairs; the hook applies them to the local reply
  (both writer paths drain them).
- **C-3 (important)**: a mid-body `CONTINUE_AND_REPLACE` body verdict was
  silently downgraded to CONTINUE; now rejected per the failure policy
  ("replace and stop" semantics are not implemented).
- **C-4 (important)**: body-phase failure reasons now reuse the request-stage
  taxonomy (`unreachable` / `bad verdict` + static detail), keeping dye
  causes and log grammar uniform across stages.
- **B-M2**: terminate replies now carry corrected framing (drain queued ops,
  strip TE, set Content-Length) like every sibling local-reply writer.
- **B-M5b/M5c**: body-mutation apply consumes the verdict (no double copy);
  `local_reply_written` is set only when a write actually happened.
- **B-M1/M4/M5d**: documented — body-phase dye recording is response-
  direction-only; handler logs are lost when the downstream future is
  dropped mid-chunk (accepted cost, noted on the ctx field); multi-handler
  composition semantics added to the trait doc.
- **A-1/A-2 (important)** + A-3..7: stale flavor-era statements in
  `skills/04-review/supply-chain/pingora-fork-branch-lock-policy.md` and the
  TLS-backends skill code block (plus five minor doc/comment/naming sites)
  updated to the fork-only world.

### Tests added from Round 2

Unit: `run_request_streamed` gate branches (no transport fact → fail per
policy; SKIP-headers install without network; install refusal → fail per
policy), mid-body CR rejection, immediate-response header carry+filter.
Integration: multi-chunk streamed upload (channel-backed body), streamed
fail-open with a dead processor (original body must reach the backend),
degenerate-path CR fail-close, and the mid-body 403 case now asserts the
immediate response's custom header.

### Recorded as accepted / deferred

- Pre-decided (h2 GOAWAY) retries rely on the proxy's `request_retry_allowed`
  backstop — sound, and load-bearing on the monotonic latch (B finding 6 /
  Round 2 B note).
- `request_body_filters` stage logs may serialize after response stages for
  early-response races (order-preserving array; dashboards should not assume
  pipeline order).
- Clean-completion streams drop with RST_STREAM instead of reading server
  EOS (cosmetic; processors may log cancellations).
- H2 downstream integration client, upstream chunked-TE assertion, and a
  BUFFERED+STREAMED co-configured chain remain deferred with rationale
  (fork-tested seams; framework cost).
