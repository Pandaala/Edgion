# Response-body pipeline — hardening backlog from the three-way deep audit

| Key | Value |
|-----|-------|
| Created | 2026-08-05 |
| Status | todo |
| Source | Three independent deep audits (correctness / adversarial-security / test-coverage) of the response-body pipeline work, run after the feature passed its final whole-branch review |
| Scope | `edgion-gateway` + the Pingora fork (`Pandaala/pingora`, branch `edgion`) |

The feature itself (async upstream response-body hook, bounded 1→N emit, clean
mid-stream terminate, AI quota termination) passed correctness and adversarial
review. The adversarial audit's verdict was that it is safe to run on the public
internet: no reachable panic, no unbounded allocation, no cross-tenant data
effect, no protocol violation, and no way to turn the injected error event into
response splitting or SSE-framing escape.

What follows is everything the three audits found that is worth doing anyway.
Nothing here is a stop-ship for the data plane. Two items are cost-control and
multi-tenancy defects that matter before this carries production AI traffic at
scale.

---

## Part 1 — Defects

### D1. A single oversized SSE event permanently disables mid-stream quota enforcement

**Severity: high (cost control bypass). Reachable by a hostile OR a merely
buggy upstream provider.**

`SseDecoder::feed_each_dyn` returns immediately once `degraded` is set
(`edgion-gateway/src/sse/mod.rs:122`), and `degrade()` (`:283`) is permanent —
nothing resets it for the life of the decoder. In `UsageExtractionProcessor::
observe_chunk` (`edgion-gateway/src/ai/usage.rs:594-603`), `quota.add_output_units()`
is called **only** from inside `feed_each`'s visitor closure.

So: one unterminated SSE event larger than `DEFAULT_MAX_PENDING_EVENT_BYTES`
(64 KiB) degrades the decoder → the visitor is never called again →
`add_output_units` is never called again → the quota worker is never notified →
`over_limit` never flips → **the stream bills freely until the request timeout.**

Triggering it does not require an attacker. A provider returning one large
tool-call payload is enough.

Why it looked covered: `sse_overflow_degrades_without_stopping_character_count`
asserts that the *character* counter survives degradation. It does — but
`add_output_chars` feeds post-hoc estimation, while `add_output_units` feeds
mid-stream enforcement. The two diverge exactly at degradation.

**Note this is not a defect introduced by this work.** The decoder's degradation
semantics are pre-existing; previously they only affected usage estimation, which
has a fallback. This work gave them a second, unfallback-ed consequence.

Fix direction (needs a product call): either keep feeding the quota from the
character estimate once degraded, or treat degradation as a fail-close condition
for that request. Doing nothing means the feature's headline guarantee has a
provider-triggerable bypass.

**Closed follow-up (D1b, 2026-08-11):** invalid UTF-8 could freeze the same
accumulator without degrading the decoder because poisoned events are dropped
before the quota visitor. The decoder now counts completed poisoned events for
its full lifetime; OpenAI quota observation tolerates one and fails closed on
the second under the existing streaming + quota + Reject gate. The regression
and review decision live in
`skills/04-review/ai-quota/sse-malformed-events-must-not-disable-enforcement.md`.

### D2. A quota termination is booked as an upstream truncation, so the credential's TPM reservation is never reconciled

**Severity: high (cross-tenant capacity leak).**

Terminate makes the `end_of_stream` callback unreachable, so
`AiAttemptResponseProcessor::on_chunk` never calls `mark_eos`
(`edgion-gateway/src/ai/dispatch.rs:777-779`). `AiAttemptRuntimeState::finalize`
then takes the `else` branch (`ai/attempt_runtime.rs:432-434`) → `SettlementOperation::Retain`
→ `settle()` returns early without issuing `Reconcile` (`:627`).

Concretely: a streaming request with `max_tokens: 4096` reserves ~4096+ tokens
against the credential's per-minute counter at admission. Terminated twenty
output tokens in, **the remaining ~4076 stay booked for the rest of the window**
(TTL is `WINDOW - elapsed + 5s`).

Terminations are a routine, operator-configured outcome — not an exception. A
handful of over-quota users can exhaust a shared credential's TPM and cause
`DeniedTpm` → `AiAttemptAdmission::Skip` **for unrelated tenants on that
credential.**

The design spec's own §3.4 forbids exactly this: *"Settlement must not depend on
the eos callback."* The user-facing quota bucket obeys it (`finalize_ai_quota`
reconciles from `charged_request_tokens`, `pg_logging.rs:195-210`); the attempt
ledger does not.

Fix direction: at terminate time the gateway knows the upstream stopped and the
actual token count is knowable, so `Reconcile(charged)` is the correct
settlement. Do **not** resurrect the eos callback (the spec forbids it) — add a
second latch set alongside `EdgionStatus::AiQuotaOverLimitTerminated` so
`finalize` can distinguish "gateway terminated this" from "upstream truncated".

### D3. The injected error event can be swallowed by a partial upstream event

**Severity: medium (the client's only diagnostic disappears). Reachable by a
hostile upstream, and by ordinary TCP segmentation.**

`UsageExtractionProcessor` is purely observational — it never rewrites `chunk`,
so upstream bytes are forwarded verbatim and the injected event is appended by
the pump at whatever byte offset the upstream's chunk boundary fell on
(`ai/usage.rs:59-65`).

If the upstream ends a chunk mid-event, the injected
`event: error\ndata: {...}\n\n` **continues that partial event** instead of
starting a new one. The client's SSE parser sees one event whose data is
malformed JSON and drops it. Token burn still stops and the stream still ends
cleanly — but the client cannot distinguish a quota cutoff from an upstream
hang-up, which is the entire reason the event is injected.

`AnthropicResponseProcessor` is immune by construction: `process_streaming`
re-emits only complete `data: …\n\n` events, so its output always ends on an
event boundary.

Fix: prefix the injected bytes with `\n\n` (a blank line is a no-op dispatch for
an idle decoder and closes a partial event otherwise), or condition the prefix
on `!self.sse.is_idle()` — the processor consumes exactly the bytes it forwards,
so `SseDecoder::is_idle()` is an accurate boundary oracle at that moment.

### D4. `failure: "incomplete_response"` is stamped on a response the gateway itself ended

**Severity: medium (corrupts the primary upstream-health signal).**

`ai/attempt_runtime.rs:436`: `let failure = (!eos).then_some(GenAiAttemptFailure::IncompleteResponse);`

Every quota-terminated stream logs `genai.attempts[].failure = "incomplete_response"`
against a perfectly healthy provider. That field's only other producer is a
genuine upstream truncation, so an operator alerting on it gets a false positive
per policy action — and the same access-log line simultaneously carries the
correct signal (`AiQuotaOverLimitTerminated`). The two contradict each other.

Fixing D2's latch fixes this at the same time.

### D5. A sealed Anthropic stream consumes the upstream forever and can never terminate

**Severity: medium (cost control gap).**

After `fail_streaming`, `self.sealed` is set and `on_chunk` returns
`*chunk = None` for every subsequent chunk — the gateway keeps reading and
discarding upstream bytes for the life of the stream while the client sees
nothing. The terminate gate is guarded by `!self.sealed`, so a sealed stream can
never be cut. Every discarded byte was paid for.

Reachable via any provider that emits one malformed event.

### D6. The sink bounds bytes but not chunk count

**Severity: low (latent; a trap for the next processor author).**

`ResponseBodySink::push` (`pingora-proxy/src/response_body_sink.rs:61-78`) caps
total bytes per batch at 1 MiB but places no limit on the number of chunks. A
processor may push ~1,048,576 one-byte `Bytes`, each becoming a separate
`HttpTask`, a separate `cache_http_task` write, and a separate wire chunk:
roughly 32 MB of `Vec<Bytes>` + 48 MB of `Vec<HttpTask>` resident, plus ~6 MB of
H1 chunked framing on the wire and a million iterations of the per-task
downstream-module filters.

No shipped processor pushes more than one ~130-byte chunk, so nothing reaches
this today.

Fix: also cap `extra.len()` (e.g. 64), and state in the doc comment that the
budget is per **batch**, not per response — total emitted bytes over a long
stream are currently unbounded; only resident memory is bounded.

### D7. One latent abort path survives in the cache-feed helper

**Severity: low (unreachable today, but it is a process-abort class).**

`migrate_end_of_stream` returns `LeadingTask::Unchanged` for every
non-`Body`/`UpgradedBody` task (`pingora-proxy/src/proxy_cache.rs:1087-1097`).
If a non-empty sink ever reached `cache_task_and_emitted_chunks` alongside
`HttpTask::Header(_, true)` or `HttpTask::Done`, the cache would receive the
end-of-stream first and then `Body(chunk, false)` — landing on
`session.cache.miss_handler().unwrap()` (`:694`, whose own comment says *"this
will panic if more data is sent after we see end_stream but should be impossible
in real world"*). That is a process abort.

Unreachable today: `upstream_filter` fills the sink only for `Body`/`UpgradedBody`,
and every filter exit drains or discards. The existing defensive unit test covers
the *delivery* path, where appending is harmless — not the *cache* path, which is
the one that panics.

Fix: skip the emitted chunks entirely when the leading task is not a
`Body`/`UpgradedBody`. One line, retires the class.

### D8. Read-only response-body filter chains never receive their end-of-stream call

**Severity: low.**

A route carrying both `ai-proxy` with `tokenQuota:Reject` and a Wasm
response-body plugin: when the quota terminate fires, the pump returns before the
`end_of_stream = true` chunk, so the Wasm plugin's
`on_response_body(end_of_stream = true)` never runs. Any read-only body filter
that finalizes on eos silently loses its last callback.

The trait comment documents that these chains still see the terminating chunk;
it does not state that they will never see the eos one.

### D9. Emitted chunks bypass `RangeBodyFilter` and the downstream `response_body_filter`

**Severity: low (not reachable from Edgion; a latent fork-API hazard).**

The leading task's body goes through `range_body_filter.filter_body(data)` and
`inner.response_body_filter(...)`; `drain_emitted_chunks` then appends the sink's
chunks after both, untouched. On a 206 whose `Content-Range` was computed by
`range_header_filter`, an emitting filter would put more bytes on the wire than
the range declares.

Not reachable in Edgion (no shipped processor emits on a cacheable/ranged route,
and Edgion implements no `response_body_filter`), but the fork exports this API
to other consumers and the asymmetry is undocumented.

### D10. Terminate closes the downstream H1 keep-alive connection

**Severity: low (efficiency, possibly intentional).**

`pingora-proxy/src/proxy_h1.rs:716` calls `session.set_keepalive(None)`. The
chunked terminator is written first, so the response is provably complete — yet
the connection is torn down, costing one extra TCP+TLS handshake per terminated
stream for H1 clients. No-op on H2.

The design justifies making the *upstream* connection non-reusable; it does not
discuss the downstream one. This looks inherited from the request-body terminate
contract rather than chosen.

### D11. Config-time validation gaps

**Severity: low.**

- A route that enables caching *and* has `tokenQuota` with `onExceeded: Reject`
  will fail closed with a mid-body `InternalError` the first time a client goes
  over quota mid-stream. The incompatibility is expressed only as a runtime
  error string; nothing rejects or warns at config-validation time.
- `identityFrom` must not be client-authorable. If an operator keys it off a
  client-supplied header, an attacker can now cut a victim's **in-flight**
  streams (previously they could only cause the victim's next request to 429) by
  burning the shared bucket. Worth a validation-time check, and worth documenting
  regardless.

### D12. Cache-stored injected content must be request-invariant

**Severity: low (latent, documentation).**

Emitted chunks are deliberately written into the cache so a later hit is
byte-identical to what the first client received — correct for request-invariant
injection. Any future processor injecting per-request or per-tenant content (a
banner, a user id, a rate-limit hint) into a cacheable response would have it
stored under the shared cache key and served to everyone.

Not reachable today: the only emitter always terminates in the same breath, and
terminate either disables the cache or fails closed. State the invariant on
`StreamingResponseProcessor::on_chunk` next to the `changes_body_length()`
precondition.

---

## Part 2 — Test plan

### Regression protection for the three already-fixed Criticals

| Fixed bug | Protected? |
|---|---|
| Cache panic from appending after end-of-stream | **Yes, twice** — two `proxy_cache.rs` unit tests fail structurally if the migration is reverted, and the cache-hit integration test panics pre-fix. Residual: reverting the migration *inside `cache_task_and_emitted_chunks` alone* is caught only by the coarse integration test. See T-P1-7. |
| Post-terminate origin bytes leaking in the same batch | **Weakest of the three.** The cross-batch half is solidly pinned. The in-batch half is protected only probabilistically — the test cannot force the batch composition it names and silently degrades into a duplicate of another test while reporting green. See T-P0-3. |
| A quota signal that could never fire mid-stream | **Yes** — `mid_stream_flush_publishes_over_limit_before_final_reconciliation` drives the real worker and never calls `finish`/`finalize`. **But** `over_limit_reports_the_final_status_without_blocking` in the same file passes with the fix fully reverted; it reads like regression coverage and is not. |

### P0 — before this runs on the public internet

- **T-P0-1 `oversized_sse_event_must_not_disable_mid_stream_quota_enforcement`** (Edgion integration, needs Redis + fixture mode). Emit one >64 KiB unterminated event, then 30+ ordinary deltas past the limit. Assert the body ends with the injected event and never reaches `[DONE]`, under a wall-clock deadline. **This test fails today — it pins D1.**
- **T-P0-2 `never_terminating_stream_is_bounded_by_quota_not_by_the_request_timeout`** (Edgion integration). An origin that never ends. Assert the response ends *measurably before* the route's request timeout, with both a lower bound (the stream really started) and an upper bound (quota, not the timeout, ended it).
- **T-P0-3 `terminate_drops_later_tasks_in_the_same_batch`** (fork, replaces the probabilistic test). Record filter-invocation count in the proxy ctx and assert it is exactly 1. Gives the in-batch half of Critical 2 a structural discriminator instead of a scheduler-dependent one.
- **T-P0-4 `client_disconnect_at_the_terminate_moment_leaves_no_stuck_request`** (Edgion integration). Abort the TCP connection before and during the terminate decision. Assert a follow-up same-identity request gets a well-formed response in bounded time, the process count is unchanged, and the gateway log has no panic. On the disconnect path `write_response_tasks` errors *before* the terminate block, so the entire terminate wrap-up is skipped — untested today, and mid-stream client disconnect is the most common event on a public AI gateway.
- **T-P0-5 `concurrent_streams_on_one_quota_identity_all_terminate_cleanly`** (Edgion integration). 16 concurrent streams on one identity. Assert all 16 are well-formed — each either complete or properly terminated — with an exact whole-set count. Multi-tenancy is the default state of a public gateway and has zero concurrent coverage today.
- **T-P0-6 `quota_terminated_stream_records_its_status_and_settles_its_bucket`** (Edgion integration; both helpers already exist). Assert the access log carries `999_12` and not `999_13`, and the Redis bucket settled consistently. Cheapest P0 here, and it is the only thing that would catch D2/D4.
- **T-P0-7 `emit_budget_boundary_is_exact`** (fork, replaces `exceeding_the_emit_budget_fails_the_response`). Exactly-at-budget must succeed; one over and split-over must fail — **with proof the filter actually ran**. The current test's single `assert_ne!` is satisfied by every harness failure, including the proxy never being reached.
- **T-P0-8 `redis_unavailable_mid_stream_does_not_hang_the_response_pump`** (Edgion integration, needs a fault-injection helper). Pause Redis mid-stream. Assert the response completes or terminates within the route timeout and the gateway does not panic.

### P1 — before production traffic at scale

- **T-P1-1** sealed Anthropic stream stops reading the upstream — **pins D5; expected to fail today.**
- **T-P1-2** emitting filter on HEAD / 204 / 304 does not corrupt framing.
- **T-P1-3** terminate on a custom-connector session fails closed — that branch has **zero** coverage today.
- **T-P1-4** emitting filter over an upgraded/WebSocket connection keeps the `UpgradedBody` tag — the design names re-tagging as a *process abort*, and only a unit test guards it.
- **T-P1-5** byte-at-a-time SSE delivery still terminates — maximizes batch boundaries and `reset_batch` calls; **also the natural home for D3's regression pin.**
- **T-P1-6** terminated H1 response closes the connection cleanly — pins D10's actual behavior so a change to it is visible.
- **T-P1-7** cache-side eos migration matches the delivery side — table-driven, includes the untested `LeadingTask::Drop` arm. Cheapest test on the list.
- **T-P1-8** `onExceeded` action resolves to the termination flag — the single line connecting operator policy to whether a live stream is cut has zero tests.
- **T-P1-9** an H2 client with a stalled flow-control window does not stall the gateway — assert a *second* stream on the same connection completes while the first is stalled.

### P2

Terminate on the very first / very last chunk; `content-length` disagreement and
unexpected trailers; zero-length and empty bodies with a processor installed;
Range request through an emitting filter.

### Existing tests to strengthen or retire

- `exceeding_the_emit_budget_fails_the_response` (fork) — replace with T-P0-7; it has no proof the code under test ran.
- `terminate_mid_batch_drops_only_the_leaked_tasks` (fork) — replace with T-P0-3; its own doc comment admits it cannot fail on an unfavorable scheduler, it just covers nothing.
- `over_limit_reports_the_final_status_without_blocking` (Edgion) — keep, but annotate that it is **not** regression coverage for the mid-stream fix; it passes with that fix fully reverted.
- `changes_body_length_tracks_every_precondition_the_terminate_gate_checks` — asserts hand-copied parity between two independently editable expressions. Strengthen by driving `on_chunk` for each constructor variant and asserting `sink.is_terminated()` matches the declaration.
- The seven `ai/usage.rs` + `ai/anthropic/response.rs` terminate tests all stage over-limit via `quota.finish(OverLimit)` — the same shape the project was already burned by. Do not rewrite them; add **one** test spanning the seam (real `AiQuotaRequest`, real SSE deltas, worker flush, terminate fires, no `finish()` anywhere) that makes the other seven trustworthy.

---

## Part 3 — Harness gaps (one-time investment, unlocks many of the above)

1. **No response-side hostile-origin script.** `test_server`'s transport-shape backends are entirely *request*-body oriented. There is no way to script an upstream that drips the response N bytes per interval, hangs mid-event, resets at a response-body byte offset, declares a disagreeing `content-length`, or appends trailers. Adding `x-up-resp-*` verbs mirroring the existing request-side offsets is a bounded change to two files and unlocks most of the hostile-upstream category. **Highest leverage item on this list.**
2. **`ai_stream_plan` is a hardcoded mode table.** Each new streaming shape costs three edits. Replacing it with a query-parameterized plan (bounded and clamped, as `/stream-trickle` already is) collapses T-P0-1, T-P0-2 and T-P1-5 into one change. Keep the named modes as aliases.
3. **No Redis fault injection.** `docker pause`/`unpause` behind `--full-test` (which already requires Docker) is the whole lift, and it unlocks quota-worker failure coverage well beyond this feature.
4. **No slow-read / stalled-window client helper.** `RawH2Conn` has manual `release_capacity` control but no helper expressing "read K bytes then stop releasing for T". About a dozen lines.
5. **The fork's sink suite is single-protocol.** Both services are `add_tcp`, driven by `reqwest` over HTTP/1.1 against a hand-rolled H1 origin — so `proxy_h2.rs`'s terminate block is covered only transitively, from another repo. Meanwhile `tests/seam/` already provides `spawn_scripted_upstream`, `spawn_scripted_h2_upstream`, a `Recorder`, and an `ExercisedUpstream` vacuity guard, and does not need openresty. Porting the sink suite onto `seam/` — or at minimum adopting `ExercisedUpstream` and adding one h2c service — is the highest-leverage fork-side investment.
6. **Doc drift, worth fixing while adjacent.** `skills/05-testing/references/test-server-capabilities.md:84-85` still lists the old `mock_mode` set and omits `quota_terminate` / `quota_terminate_force_sse` and the fixture's deliberate `content-length`. Anyone writing the tests above reads that page first.
