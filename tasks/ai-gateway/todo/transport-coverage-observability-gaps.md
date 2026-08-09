# Transport Coverage — Observability Gaps We Cannot Test Yet

| Key | Value |
|-----|-------|
| Created | 2026-08-03 |
| Status | todo |
| Context | Fell out of the request-body transport seam work (`tasks/archive/completed-tasks/request-body-capability-followups/08-ext-proc-streamed-request-body/`, contract `09-*.md`) and the test-harness build-out that followed it. |

> **This file is the evidence record. The actionable work lives in
> `tasks/todo/important_test/`, where gaps 1-8 are grouped into four classes by
> *where the change goes* and *what method it needs*:**
>
> | File | Gaps | Where the change goes |
> |---|---|---|
> | `01-harness-wire-event-primitives.md` | 2, 4 | **done 2026-08-04, file removed** — observation variant + emission steps landed in the fork harness |
> | `02-request-lifecycle-observation.md` | 1, 3 | **done 2026-08-04 (Route A), file removed** — shared statics replaced by per-request completion handles in the fork harness; both H2c promptness cells un-skipped. Route B (a production-side lifecycle signal) stays a deferred design decision, recorded under gap 1 |
> | `03-fork-layer-coverage-for-unreachable-paths.md` | 5, 6 | fork test suite — Edgion cannot route to these at all |
> | `04-fix-the-product-before-writing-the-test.md` | 7, 8 | the product, not the tests — do NOT test today's behavior |
>
> Gaps 2, 3, 4, 9 and 10 are historical records, not tasks: 2 and 4 were fixed
> on 2026-08-04 (with gap 2's central factual claim corrected in place), 3 was
> fixed on 2026-08-04 (per-request completion observation), 9 was fixed on
> 2026-08-03/04 (with four of its original claims corrected in place), and 10
> documents two contracts the suite corrected rather than discovered. Gap 1
> keeps only its deferred-decision residual.

These are behaviors we **want** asserted but currently **cannot** assert, in
either the Pingora fork's own suite or Edgion's integration suite. Each entry
names why it is unobservable and what would unblock it, so a future attempt
starts from the constraint rather than re-deriving it.

Nothing here is a known defect. They are blind spots — which is exactly why
they are worth recording: an untested behavior is one that can regress
silently.

## 1. "The proxy finished the request" has no wire form — attribution FIXED 2026-08-04, wire form deferred

> **Status: the harness half is done; the production half is a deferred
> decision.** The shared `FINAL_EOS_*` / `OBSERVED_RETRY_*` / `COMPLETED_*`
> statics were replaced by per-request completion handles: a test registers an
> observation (`observe_completion()` in `pingora-proxy/tests/seam/harness.rs`),
> sends its id as `x-observe-completion`, and `ProxyHttp::logging` fulfils the
> handle with a `CompletionRecord` (finish instant, final EOS count, final
> retry flag). Attribution is structural — the test owns the handle, the entry
> dies with it — so the observation works even on a multiplexed downstream
> connection, which is what un-skipped gap 3's cells. Sensitivity was verified
> by mutation: a 3s park injected into each pump's terminate arm made every
> consumer of the handle fail as required (mutate, observe, restore).

**Residual — deliberately deferred, not forgotten:** the proof is still an
application-side callback, not a wire fact or a production signal. A real
proxy-side lifecycle observable (Envoy's position: stats that tests poll) is a
genuine API decision in the fork with value beyond tests — request-lifecycle
observability is something operations wants anyway. Revisit when the gateway
gains lifecycle observability for operational reasons; the test-side residual
then closes for free. Do not add a production metric for tests alone.

## 2. H1 "the proxy cancelled this leg" is unobservable — FIXED 2026-08-04

> **Status: fixed, with the entry's central factual claim corrected.** The
> harness now records `UpEvent::PeerConnReset` whenever any H1 read fails with
> `ECONNRESET` (the connection-level sibling of the H2 `PeerReset`), and the
> bodyless H1-upstream cells assert the proxy ended the connection.

**Correction — the proxy does NOT reset; it sends a clean FIN.** The original
claim "when the proxy abandons an H1 upstream leg it resets rather than sending
FIN" was written while the event was unobservable, and observation disproved
it: across every run on macOS the abandoned leg closes with `Ok(0)` (FIN), ~1ms
after the fail-closed 500. Whether FIN or RST appears depends on whether the
pump drained the upstream's early 200 before the teardown closed the socket —
an implementation accident, not the contract. The strengthened cells therefore
assert "the proxy ended the connection (FIN **or** RST) during the recording
window", which is exactly the claim that matters: a proxy that pooled the
connection for reuse produces neither event, and a `mem::forget` mutation of
the non-reuse drop in `pingora-proxy/src/lib.rs` made both cells fail as
required (mutate, observe, restore).

**Surfaced along the way — a latent flake in the H2-upstream bodyless cells.**
The bodyless end signal races with the deferred HEADERS write in the h2 client:
when it coalesces, END_STREAM rides the HEADERS frame, the request half is
closed before the proxy fails the request, and there is nothing left to cancel
— so the cells' unconditional RST_STREAM wait timed out whenever that ordering
occurred (observed ~2/11 parallel runs once the strengthened H1 cells shifted
scheduling; latent on the clean tree too). The assertion now branches on the
recorded `headers_eos`: the non-coalesced form still demands the cancel, the
coalesced form announces itself (same visibility rule as `skip_combo!`).

## 3. Promptness cannot be asserted on an H2c downstream — FIXED 2026-08-04

> **Status: fixed.** Both H2c cells of
> `terminate_is_prompt_and_cancels_the_upstream` now run instead of skipping.
> The lower bound is unchanged (the recorded upstream `ReqHeaders` instant);
> the upper bound is the per-request completion observation from gap 1:
> `logging` only runs once the proxy finished the request, and
> `x-observe-completion` attributes that fact to one stream even on a shared
> connection, where time-to-EOF (the H1 cells' wire observable) does not
> exist. `finished_at` is stamped proxy-side, so the measurement is immune to
> test scheduling. The H2c client also drains the local reply tolerating both
> legitimate stream ends — END_STREAM, or the `RST_STREAM(NO_ERROR)` the proxy
> sends because the client's request half is still open (gap 9's section 8.1
> premise: h2 may surface that reset as a read error that discards buffered
> DATA) — while any other reset code fails the cell as a terminate dressed up
> as a proxy error.

## 4. `PeerGoaway` is recorded but never asserted — FIXED 2026-08-04 (one residual)

> **Status: the important half — the proxy's GOAWAY handling — is now
> covered.** Two scripted-upstream steps emit a GOAWAY mid-exchange
> (`H2UpstreamStep::GoawayThenOk200` for the graceful `NO_ERROR` shape,
> `H2UpstreamStep::AbruptGoaway(code)` for the error shape; both sequence the
> GOAWAY into the frame queue ahead of anything the step writes afterwards),
> and two matrix scenarios assert the proxy's actual reaction: a graceful
> GOAWAY completes the in-flight stream to the client AND evicts the
> connection (the follow-up request provably opens a fresh one — verified
> sensitive by removing the GOAWAY step, which makes the eviction assertion
> fail on the reused connection); an error GOAWAY fails the in-flight request
> as a 502 with exactly one attempt and no silent retry.

**Residual — the `PeerGoaway` recording arm itself is still never driven.**
That variant records a GOAWAY the PROXY sends to the upstream, and observation
shows pingora's h2 client tears its connection down with a plain TCP close in
these flows — no GOAWAY frame, so the arm stays unexercised. Driving it would
require the proxy to start sending GOAWAY on client shutdown, a product change
nobody has asked for. Not worth a task; recorded so nobody re-derives it.

## 5. CONNECT framing variants are unreachable from Edgion

**Where:** Edgion integration suite.

**Problem:** the fork coerces a non-`Ordinary` upstream body disposition to
`Ordinary` for CONNECT requests, and we would like that guard asserted
end-to-end. But Gateway API has no CONNECT route type, so no Edgion
configuration can route a CONNECT request — the path is unreachable from this
layer. The upgrade half of the same guard **is** reachable and is covered.

**Unblock:** nothing short of a non-Gateway-API entry point. The fork's own
suite is the right home for this guard; record it there if it is ever wanted.

> **Update (2026-08-04):** pinned in the fork as
> `seam::single::bodyless_does_not_half_close_a_connect_tunnel` — but via a
> plain H2 CONNECT (H2c downstream → H2 upstream), not the H1 framing shapes
> this gap originally imagined: observation showed an H1 CONNECT is not
> constructible in the fork's suite at all (authority-form parsing needs
> `patched_http1`, which does not compile against stock httparse; and the H1
> upstream serializer panics on an authority-form URI). Details and the full
> correction:
> `tasks/todo/important_test/03-fork-layer-coverage-for-unreachable-paths.md`.

## 6. The custom-connector pump has no end-to-end coverage

**Where:** both suites.

**Problem:** `proxy_custom.rs` received the same seam wiring as the H1/H2 pumps
(action hook, fail-closed disposition, retry gate), but no Edgion route reaches
it, and the fork's harness has no custom-connector session to drive. Its
behavior is currently guaranteed by code review alone.

**Unblock:** a fork-side test harness that constructs a `Custom` session. Worth
doing if the custom connector ever carries production traffic; not before.

## 7. Downstream request-body idle timeout — FIXED 2026-08-04

> **Status: fixed in the fork.** The H2 server session now honors a per-chunk
> `read_timeout` on downstream request-body reads (`v2/server.rs`
> `read_body_bytes`), and the unified `HttpSession::set_read_timeout` H2 arm is
> no longer a no-op — so the timeout Edgion already sets on every request
> (`pg_early_request_filter.rs`, from
> `spec.httpTimeout.client.readTimeout`, default 60s) now bounds H2 uploads the
> way it always bounded H1. Applied uniformly to gRPC (decided 2026-08-04; a
> content-type exemption was rejected as attacker-bypassable). Both decided
> directions are pinned by fork tests:
> `test_read_body_timeout_releases_stalled_upload` and
> `test_read_body_timeout_spares_progressing_upload`. Decision record:
> `skills/04-review/h2-grpc/h2-downstream-body-read-timeout.md`.

## 8. Terminate-driven upstream RST_STREAM is not rate-limited — ACCEPTED 2026-08-04

> **Status: accepted tradeoff, decision recorded.** No terminate-heavy body
> policy ships today, and the correct mitigation is Edgion plugin ordering
> (reject in the request stage, before the upstream stream opens), not a proxy
> budget. Concrete re-evaluation triggers (a terminate-heavy policy shipping,
> or an origin GOAWAY incident traced to Edgion reset volume) and the rejected
> fixes — including any test pinning "N terminates → N resets" — are recorded
> in `skills/04-review/h2-grpc/terminate-upstream-rst-not-rate-limited.md`.

**Original problem (kept for context):** a client that reliably trips a body
policy drives a HEADERS→RST_STREAM cycle per request against the upstream —
the Rapid Reset shape, aimed at the origin. Because an H2 upstream connection
is shared across tenants, an origin enforcing a post-CVE-2023-44487 RST budget
could GOAWAY it for everyone.

## 9. Benign RST_STREAM(NO_ERROR) after a complete, unframed H2 response

> **Status 2026-08-03, after an independent deep review: three claims in the
> original entry were wrong.** They are corrected in place below rather than
> deleted, because each was acted on and the corrections are the useful part.
> Work is in flight on both sides; this section will need a final pass once it
> lands.

**The defect is real.** The fork classifies a post-response upstream
`RST_STREAM(NO_ERROR)` as benign only once its EOF latch is set, and the latch
originally had three sources: `END_STREAM` on HEADERS, `is_end_stream()`
observed at a poll, or a declared `Content-Length` fully received. An upstream
that sends `send_response(.., false)` → `send_data(body, true)` →
`RST_STREAM(NO_ERROR)` with **no** `Content-Length` sets none of them, so the
read errors and the client gets a **502 in place of a complete 200**. The shape
is not exotic: a streaming H2 origin has no reason to send `Content-Length`,
and RFC 9113 §8.1 blesses cancelling the request after a complete response.

**Correction 1 — the proposed fix was not implementable as written.** The entry
suggested "latch EOF on the DATA frame that carries `END_STREAM`". h2 0.4.13's
`RecvStream` exposes only `poll_data` (which returns bytes with **no per-frame
END_STREAM flag**), `poll_trailers`, `is_end_stream()` and `flow_control()`.
Nothing in that API can tell you whether the chunk you just received carried
END_STREAM. The implemented fix instead scans **frame headers on the wire**
before the bytes reach h2 (`pingora-core/src/protocols/http/v2/end_stream_watch.rs`),
which sidesteps the API limitation entirely.

**Correction 2 — the evidence is not always destroyed.** `State::recv_reset`
begins `Closed(..) if !queued => {}` — a reset arriving at an already-`Closed`
stream is a **no-op** and `is_end_stream()` keeps reporting true. The clobber
happens only in states such as `HalfClosedRemote`, i.e. **while the client is
still uploading its request body**. That is narrower than "the reset erases the
record", and it is also exactly §8.1's premise, so the narrowing does not make
the defect rarer — but it did admit a fix the original analysis dismissed.

**Correction 3 — the integration case was not testing the shape at all.** The
alternating pass/fail between the `h1_to_h2` and `h2_to_h2` cases was **a test
backend bug, not the fork's**. `edgion-tests/src/server/transport/h2.rs` called
`send_data(body, true)` and then `send_reset(..)` with no await between, and in
h2 `send_reset` both `clear_queue`s the stream's still-queued frames and
returns early emitting **no RST at all** when the stream is already closed and
its queue empty. Measured over 960 standalone runs, the intended shape occurred
**zero times**: every run was either "HEADERS+DATA destroyed, only RST sent"
(→ 502) or "complete response, no reset" (→ 200). A coin flip, not a product
signal. Worse, `Behavior::Echo` drains the whole request body before responding,
which puts the server stream in `Closed(EndStream)` and suppresses the reset
outright — so that backend behavior **cannot** express §8.1, whose premise is an
in-flight upload.

**Correction 4 — the read-half fix alone is not sufficient.** Because the shape
requires an in-flight upload, the upstream write fails too, and `proxy_h2`'s
`biased select!` polls the upload pump first — so the write error can still win
the race and produce a 502 even when the complete response is in hand. Fixing
the read half without the write half leaves the end-to-end outcome a coin flip.

**Standing lesson, independent of this defect:** a test whose backend cannot
emit the shape it names will alternate, and an assertion recorded *before* the
action it claims to observe cannot fail. Both were present here. Verify the
wire, not the intent.

## 10. Two contracts the transport suite corrected rather than discovered

Recorded because both were originally written as the opposite expectation, and
the next person to read those cases will otherwise re-litigate them.

**HTTP/1 request pipelining is not supported, deliberately.** Two heads written
in one segment: the first request is answered normally and the connection is
then closed with a clean FIN; the second is never forwarded upstream — no access
log entry, no recorder event — and the client is expected to resend it. This is
pingora's own design (`HttpSession::reuse` in
`pingora-core/src/protocols/http/v1/server.rs` refuses reuse when
`has_bytes_overread()`, "the pipelined request left unread, and the client can
attempt to resend"). `client_pipelines_second_request` now asserts the safety
property that makes it sound — never mis-paired, never smuggled, always
resendable — instead of an ordering that cannot occur.

**`Expect: 100-continue` is answered only where a plugin buffers the body.**
`capture_request_body`
(`edgion-gateway/src/plugins/runtime/session_adapter.rs`) writes the interim and
strips `Expect`; that path is covered by `EdgionPlugins/Dsl`. On a plainly
proxied route nothing answers the expectation, and pingora's H1 upstream client
consumes any interim the origin sends rather than relaying it, so no interim can
reach the client by either route. The request head still goes upstream
immediately and the body is accepted when the client stops waiting and sends it,
which is what `client_expect_100_continue` now asserts.

Neither is filed as a defect. If either is ever *wanted* — a gateway-generated
interim on unbuffered routes, or genuine pipelining — that is a feature
decision, and this section is the starting point for it.

## Related, and deliberately NOT recorded here

The Pingora fork's own `test_basic.rs` / `test_upstream.rs` require a local
openresty binary and are inert for us — we do not run them. They are not a
coverage gap to close; Edgion's integration suite is the regression net for
everything we actually deploy. Do not re-add "run the openresty suites" as a
task.
