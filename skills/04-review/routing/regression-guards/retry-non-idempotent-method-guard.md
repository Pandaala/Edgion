---
name: retry-non-idempotent-method-guard
description: Use when reviewing findings that connection-error retries replay non-idempotent (POST/PATCH/...) request bodies on a reused connection without consulting the HTTP method; covers why the guard lives in Case 2 + an annotation, not the standard CRD.
status: fixed
finding_id: route-01
closed: 2026-06-16
knowledge_type: regression-guards
---

# Reused-connection retry replays non-idempotent bodies — FIXED (method-safety guard)

## Conclusion

The reused-connection retry path (`error_while_proxy` Case 2,
`edgion-gateway/src/routes/http/proxy_http/pg_error_while_proxy.rs`) now consults the
HTTP method before replaying a buffered request body. Non-idempotent methods
(POST/PATCH/CONNECT, and all gRPC since it is POST at the HTTP layer) are **not**
retried on a reused keepalive reset by default; idempotent methods
(GET/HEAD/PUT/DELETE/OPTIONS/TRACE per `http::Method::is_idempotent()` / RFC 9110)
still are. A route opts back in with `edgion.io/retry-non-idempotent: "true"`.
The same method, replay-backing, attempt-budget, and deadline gates also veto
already-decided `InvalidH2` and `H2Downgrade` errors. These local protocol
outcomes do not establish that the origin skipped the request. The AI attempt
dispatcher explicitly clears a preexisting retry decision when it returns
`Final` or fails.

Re-report fix suggestions of the following forms are **not accepted**:
- "Add a `retriable_methods` / `allowNonSafeRetry` field to the `HTTPRouteRetry` CRD" —
  `HTTPRoute` is the **standard** Gateway API CRD (`gateway.networking.k8s.io`, vendored
  under `config/crd/gateway-api/`). It is a structural schema, so the API server prunes
  any field not declared upstream → a new spec field is a silent no-op. The knob MUST be
  an annotation (the Gateway-API-blessed vendor-extension mechanism), not a CRD field.
- "Also guard `fail_to_connect`" — that path never wrote request bytes upstream (connection
  never established), so a retry cannot double-execute. Guarding it would needlessly suppress
  safe retries.
- "Also guard Case 1 (status-code `retry.codes`) and Case 1.5 (per-try timeout)" — those
  fire only under an **explicit** operator-configured retry policy (`retry_configured()`),
  i.e. the operator opted into retrying. Case 2 and Pingora's ambiguous
  already-decided H2 protocol outcomes can retry without per-route policy;
  both now apply the method-safety guard. This does not change the explicit
  status or per-try-timeout policy.

## Core Rationale

Pingora's native retry prefix is bounded by `BODY_BUF_LIMIT` (64 KiB). Edgion
can also register its canonical bounded request `BodyStorage` for replay, so
the method guard is not limited to small native-buffer bodies. Replaying a
non-idempotent request after a reused keepalive reset can duplicate an origin
operation even when its complete bytes are available. Replay backing proves
that bytes can be sent again; it does not prove that the origin did no work.

The guard is a single conjunct on `can_retry`:
`method_retry_ok = session.req_header().method.is_idempotent() || ctx.retry_allow_non_idempotent()`.
`decide_reuse` is a no-op for an incoming `Decided(true)`. Pingora 0.9 can
produce that state for local H2 `GOAWAY(PROTOCOL_ERROR)` as `InvalidH2`, even
when an origin may already have processed a POST. The override must explicitly
clear the decision when the method, backing, budget, or deadline gate fails.
Protocol-qualified `REFUSED_STREAM` and peer GOAWAY decisions remain subject
to the separate Pingora-decided ceiling. Their safety depends on the precise
stream condition, not on every error called `H2Error` being safe.

### P090-008 transport policy table

The inspected baseline is Edgion's locked Pingora revision
`edgion_v0.9.1@e5f4287be713d01ffd444544251bf75c9d00062a`, with `h2` 0.4.20.
The table's fork-dependent rows were re-verified by source inspection at that
revision on 2026-11-06 after the fork was rebased onto upstream 0.9.1:
`handle_read_header_error` (`pingora-core/src/protocols/http/v2/client.rs`) and
`RetryType` (`pingora-error/src/lib.rs`) are unchanged from the earlier verified revision `0b0d8bba`, and
`h2` 0.4.20 `Inner::recv_go_away` (`src/proto/streams/streams.rs`) still errors
only locally initiated streams above `last_stream_id`. Historical: P090-008
originally inspected revision `0b0d8bba9609655bfd7db0fe0add6818bc0ee937`
(`edgion_0.9.0_v1`) with `h2` 0.4.19. The official
`6fa38359e7200561a0794311eaef31a01857a515` default-hook change, an ancestor of
both revisions, is a re-evaluation trigger, not a requirement to copy its
method blanket into Edgion's override.

| Failure / incoming decision | Ordinary HTTP or GRPC route | AI attempt plan |
| --- | --- | --- |
| Fresh TCP establishment failure | No request was sent; existing connect policy and structural replay gate apply | Predispatch reservation and AI successor policy apply |
| Real IO error, `ReusedOnly`, fresh upstream connection | No implicit retry | Ambiguous failure requires explicit non-idempotent opt-in and AI gates |
| Real IO error, `ReusedOnly`, reused connection | Idempotent method or annotation opt-in, usable backing, remaining route attempts, live deadlines | Same AI ambiguous-failure opt-in and dispatch gates; no implicit reuse fallback |
| Local H2 protocol error / downgrade, `Decided(true)` | Method/opt-in, backing, route attempts and deadlines can revoke the decision even on a fresh connection | Ambiguous class; AI `Final`, budget denial and errors clear the incoming decision |
| Peer `RST_STREAM(REFUSED_STREAM)` | Preserve the protocol-qualified decision, including POST; separate Pingora-decided ceiling and final structural gates still apply | Conservative ambiguous class; opt-in and AI dispatch gates apply |
| Peer `GOAWAY(NO_ERROR)` for a stream above `last_stream_id` | Preserve the protocol-qualified decision, including POST; separate ceiling and final structural gates still apply | Conservative ambiguous class; opt-in and AI dispatch gates apply |
| `Decided(false)` without an explicit retry branch | Remains false | Remains false unless the AI policy explicitly selects a successor |
| Explicit retryable status / `backendRequest` timeout | Preserve explicit operator policy, including POST; attempts, deadlines and backend retryConstraint apply | Existing AI status/timeout transition and budgets apply |
| Total request or gRPC deadline expired | No new origin attempt; `upstream_peer` checks again before selecting a peer and after backoff | Same deadline boundary |
| Final response selected by the bounded head processor or written downstream | No successor; the gate runs before product retry bookkeeping and again at dispatch | No scheduler advance or retry-budget consumption after this gate |
| Frozen `Never`, native truncation, unavailable registered replay, unsupported backing | Final dispatcher revokes any retry decision | Same final structural veto |

`H2Error` alone is not evidence that processing did not occur. At this pinned
revision, the pre-decided safe paths originate from the peer-qualified
REFUSED_STREAM or GOAWAY handling in `pingora-core/src/protocols/http/v2/client.rs`.
`h2::proto::streams::recv_go_away` errors locally initiated streams above the
peer's last-stream ceiling; lower streams are not swept into that decision.
See [RFC 9113 section 8.7](https://datatracker.ietf.org/doc/html/rfc9113#section-8.7).
An origin that lies about non-processing violates that protocol guarantee;
neither buffering nor H1 accepted-write accounting can repair that violation.

`RequestRelayRetryState::LiveUnread`, `NativeCapturing`, and
`RegisteredReplay` permit another structural attempt. `NativeCapturing` can
be a replayable prefix followed by unread live bytes; it need not be a full
cached document. `Disabled`, `Unsupported`, `NativeTruncated`, and
`RegisteredUnavailable` do not. Bodyless requests still obey a frozen `Never`.

### Acceptance evidence and limits

Historical: every test run recorded in this section executed against fork
revision `0b0d8bba9609655bfd7db0fe0add6818bc0ee937` ("the pinned fork" below).
The 2026-11-06 re-verification at `e5f4287be713` was source inspection only and
did not re-run these suites; the cited fork tests
(`held_head_and_body_cross_batches_until_release`, `final_statuses_forbid_retry`,
`retry_guard_ignores_1xx_but_not_101`) still exist at that revision.

`HTTPRoute/Transport` adds 65 `retry_policy__*` cases: 64 use the shared real
Gateway/Controller topology and raw H1/H2 origins; one exercises the pinned
fork connector directly against the same H2 origin. The scripts distinguish
`ReqHeaders`/`ReqData` from `EffectApplied`: only a complete-body application
operation emits the latter, with the input length and SHA-256 digest. Safe
REFUSED_STREAM and excluded-stream GOAWAY failures commit no operation before
rejecting the request. TCP-reset/status/timeout scripts deliberately commit
before failing, making duplicate operations observable under explicit opt-in.

The matrix covers both downstream protocols and both upstream protocols,
recorder-proven fresh and reused connections, default and opted-in POST,
route-attempt exhaustion, native truncation, registered 128 KiB replay, streamed
non-replayability, absent bodies, explicit status and per-try timeout policy,
total deadline, and an exact pre-decided ceiling.
Final outcomes, exact request/effect counts and bounded no-late-request
windows are required; missing upstream faults or dropped recorder events fail.

`direct_fork_refused_sibling_health` configures only its local test connector
with two stream slots and proves a real REFUSED_STREAM becomes
`H2Error / Decided(true)` while an active sibling on that same origin connection
finishes with 200, the exact body, and clean EOS. Recorder ordering proves
the sibling response follows the fault, and the origin closes before the case
returns. The Gateway retains `PeerOptions::new().max_h2_streams == 1`; its
peer builder does not override this. Thus the test proves the fork's protocol
isolation, not concurrent upstream multiplexing in today's Gateway. It adds
no production stream-limit setting. The direct origin host may be overridden
with `EDGION_TEST_TRANSPORT_H2_HOST`; local verification uses shared port 30031.

The source-unit matrix separately exercises incoming `Decided(true)` and
`Decided(false)` and ambiguous `InvalidH2`/`H2Downgrade` with an unfrozen,
disabled backing. The pure guard table covers eligible GET and independent
method/backing/attempt/deadline vetoes; real streamed routes cover frozen
`Never`. This direct callback coverage is needed because
the shared h2c backend is H2-only: Pingora can veto downgrade before the hook,
so that origin cannot honestly prove an eligible H2-to-H1 downgrade.

Fresh targeted unit runs on 2026-10-05 passed the Gateway override's 11 tests,
the AI dispatch module's 25 tests, and the route-budget module's 3 tests.
`ai_failure_classification_excludes_control_flow_and_non_upstream_errors`,
`ai_final_and_dispatch_error_clear_incoming_retry_decision`,
`ambiguous_failure_replays_only_when_explicitly_requested`, and
`dispatch_retry_budget_denies_and_counts_each_dispatch_once` pin the AI
classification, terminal verdict, opt-in, and dispatch-budget boundaries.
The first-attempt request/gRPC deadline tests verify rejection before plan
mutation. These are unit/call-chain evidence, not controlled-provider replay
counts.

The same pinned fork's original unit tests were run from a temporary source
archive, without editing fork sources: `response_pipeline::tests` passed 24
tests (one existing benchmark ignored), `retry_guard_tests` passed 3, and
`request_relay::tests` passed 21 (one existing benchmark ignored).
`held_head_and_body_cross_batches_until_release` asserts the retry gate is
closed while the final head is still held and no writer output exists;
`final_statuses_forbid_retry` and `retry_guard_ignores_1xx_but_not_101`
pin final versus informational commitment. Relay tests pin frozen replay
policy, registered backing, streamed denial, and prefix truncation. These
private-state unit results complement the real-origin matrix rather than
claiming a second Gateway topology or origin-level Hold experiment.

Fresh behavioral execution on 2026-10-05 passed `HTTPRoute/Transport` 145/145:
all 65 new cases executed, and the original 80 entries retain their 68 executed
cases and 12 explicit unsupported-combination skips. No skip counts as proof
of its unsupported transport shape. `HTTPRoute/Backend/InformationalRetry`
passed 1/1; `GRPCRoute/TimeoutRewriteWire` passed 3/3, including native POST
retry with three origin attempts and decreasing `grpc-timeout`; and
`EdgionBackend/Resilience` passed 3/3, including
`committed_sse_disconnect_never_falls_back` with zero fallback hits.
`HTTPRoute/Backend/PerTryTimeout` passed 11/11 with the local adaptation below;
`midbody_timeout_not_retried` proves an already-written 200 has one origin
attempt and no retry, while `deadline_transition_not_retried` distinguishes
per-try timeout from the remaining total deadline.

The canonical integration wrapper stopped at its global macOS loopback-alias
preflight, before behavioral tests: this host lacks `127.0.0.2` through
`127.0.0.7`. Verification used the existing shared startup utility and registered
suite runner. The original PerTryTimeout run passed 9/11; the two sticky tests
failed on connection establishment to the unavailable `127.0.0.2:30001`.
For the final 11/11 run, only that suite's non-strict EndpointSlices were
temporarily adapted through the same Controller to the shared server's existing
`127.0.0.1:30001` and `127.0.0.1:30002` sockets. The original slice was then
restored and the temporary second slice deleted. Repository fixtures and
host networking were unchanged; the raw dual-loopback fixture was not
claimed as passing on this host.

The transport client/coverage guards passed 12 unit tests and the origin-script
parser passed 10. Workspace format, check and clippy passed, as did agent-doc,
SSA-force, metrics-inventory, Pingora-source, Gateway API transform-contract,
unit-test-layout, English-only, and resource-purity guards. Full-workspace
tests were not run. P090-008's result is an investigated, justified no-change
production policy with the acceptance gap closed through test-only changes.
No production retry logic, public visibility, or new route knob was added.

The opt-in flag is a runtime-only field (`parsed_allow_non_idempotent_retry`,
`#[serde(skip)] #[schemars(skip)]`) on `HTTPRouteRule` / `GRPCRouteRule`, parsed from the
route-level annotation in each `preparse()` and stamped on every rule — the same mechanism
as `edgion.io/forward-raw-path`. preparse re-runs gateway-side, so only the annotation
travels over conf_sync, never the derived field.

## Re-evaluation Triggers

- Pingora exposes a per-attempt hook that distinguishes "body actually replayed" from
  "retry decided" — could tighten the guard to only fire when a body was buffered.
- A future requirement to make the guard per-rule (not per-route) — the annotation is
  currently route-level, applied to all rules.
- A Pingora change to H2 error classification or retry decisions — recheck
  which outcomes prove the stream was not processed.

## Reference Cases

- Original task: `tasks/todo/route-01-retriable-methods-non-idempotent-guard.md` (promoted from
  review finding `route-retry-non-idempotent-methods`).
- Sibling per-try-timeout decisions: `routing/per-try-timeout-via-backend-request.md`,
  `routing/per-try-timeout-test-coverage.md`.
- Annotation reference: `skills/02-features/05-annotations/references/annotations-metadata.md`
  (`edgion.io/retry-non-idempotent`).
- P090-008: current-revision policy investigation and controlled-origin acceptance.
