# Gateway callback dispatch inventory

Source baseline: `7ef77da08`. Status: dispatch inventory inspected; delegated
implementation coverage remains in [INDEX](INDEX.md). This is not a completion
claim for HTTP, other transports, plugins or dependency internals.

Sources: `edgion-gateway/src/routes/http/proxy_http/mod.rs` and the locked Edgion
Pingora fork's `pingora-proxy/src/proxy_trait.rs` at
`1aaa45cc70430bc782990bc20ddee3fef36d6b99` (Cargo.lock). Count only declarations
inside ProxyHttp, excluding the file's separate Future::poll implementation.
There are 52 source methods: 30 overrides and 22 inherited methods. Two inherited
upstream-module methods are feature-gated, so 52 is not a compiled-method count.

Source method sets revalidated at `2d0b7166a`: both tables still exactly match
the Gateway overrides and locked fork trait declarations (30 + 22). The two
feature-gated declarations remain included. This refresh checks inventory
consistency only; it does not rerun historical runtime tests or extend the
reachability and downstream coverage claims below.

## Edgion overrides

Paths below are relative to `routes/http/proxy_http/`. Each row records current
dispatch or inline work, not a claim that its entire reachable graph is reviewed.

| Method | Direct target or inline work |
| --- | --- |
| `new_ctx` | pg_new_ctx::new_ctx |
| `persist_connection_context` | Conditional boxed unit token from keepalive-initialized flag |
| `on_connection_reuse` | Sets keepalive-initialized flag |
| `upstream_peer` | pg_upstream_peer::upstream_peer |
| `init_downstream_modules` | pg_init_downstream_modules::init_downstream_modules |
| `request_filter` | pg_request_filter::request_filter |
| `early_request_filter` | pg_early_request_filter::early_request_filter |
| `request_body_filter_action` | pg_request_body_filter_action::request_body_filter_action |
| `request_trailer_filter` | pg_request_body_filter_action::request_trailer_filter |
| `request_relay_plan` | Reads monotonic streamed-body latch; selects streamed/ordinary |
| `response_head_may_hold` | pg_response_head::response_head_may_hold |
| `upstream_request_filter` | pg_upstream_request_filter::upstream_request_filter |
| `upstream_response_filter` | pg_upstream_response_filter::upstream_response_filter |
| `upstream_response_header_filter_event` | Records non-interim EOS fact, then same upstream header processor |
| `response_filter` | pg_response_filter::response_filter |
| `response_head_commit_plan` | pg_response_head::response_head_commit_plan |
| `response_head_will_commit` | pg_response_head::response_head_will_commit |
| `response_head_hold_boundary` | pg_response_head::response_head_hold_boundary |
| `response_head_hold_outcome` | pg_response_head::response_head_hold_outcome |
| `upstream_response_body_filter` | pg_upstream_response_body_filter::upstream_response_body_filter |
| `upstream_response_body_filter_event` | pg_upstream_response_body_filter::upstream_response_body_filter_event |
| `upstream_response_trailer_filter` | Dispatches upstream trailers, then Anthropic conversion compaction |
| `response_trailer_filter` | Transcoder-only trailer clear and terminal Bytes take under lock |
| `logging` | pg_logging::logging |
| `error_while_proxy` | pg_error_while_proxy::error_while_proxy |
| `fail_to_connect` | pg_fail_to_connect::fail_to_connect |
| `fail_to_proxy` | pg_fail_to_proxy::fail_to_proxy |
| `suppress_error_log` | Returns local-reply-written state |
| `request_summary` | Transport summary plus optional standard trace ID |
| `connected_to_upstream` | pg_connected_to_upstream::connected_to_upstream |

## Inherited methods

These are inspected default bodies, not a proof of production reachability or
absence of allocation throughout their callees.

| Method | Default behavior / boundary |
| --- | --- |
| `init_upstream_modules` | No-op; upstream_modules feature gate |
| `allow_spawning_subrequest` | false; does not prohibit proxy-core background subrequests |
| `request_body_filter` | No-op; current action override bypasses the trait's legacy bridge |
| `request_cache_filter` | No-op; does not enable fork caching |
| `cache_key_callback` | Panics unless overridden; contract requires override if caching is enabled |
| `cache_miss` | Delegates to session.cache.cache_miss |
| `cache_hit_filter` | Ok(None), no forced freshness override |
| `proxy_upstream_filter` | Ok(true), continue upstream |
| `response_cache_filter` | Uncacheable with default custom reason |
| `cache_vary_filter` | None |
| `cache_not_modified_filter` | Delegates to core conditional filter |
| `range_header_filter` | Delegates to range filter with max ranges Some(200) |
| `adjust_upstream_modules` | Ok(()); upstream_modules feature gate |
| `custom_forwarding` | Ok(()) |
| `downstream_custom_message_proxy_filter` | Returns supplied Bytes by move |
| `upstream_custom_message_proxy_filter` | Returns supplied Bytes by move |
| `response_body_filter` | Synchronous Ok(None); direct call creates no hook future and requests no delay; call-site boundary clarified below |
| `suppress_proxy_warn_log` | false; eleven call sites inspected; ownership decision deferred below |
| `should_serve_stale` | True only for an error with Upstream source; cache-gated contract |
| `is_purge` | false |
| `purge_action` | Delete; consulted when is_purge is true |
| `purge_response_filter` | Ok(()) |

### Ordinary HTTP cache reachability

Revalidated at `4ff20ab72` against the same locked fork. Session::new constructs
HttpCache::new with Disabled(NeverEnabled), no inner cache owner and no bypass.
The current Gateway inherits the no-op request_cache_filter and false is_purge.
Inspection of Gateway cache-field consumers found only unrelated TinyUFO,
plugin-generation and Wasm OCI cache owners, not writes to the proxy Session
cache. Registered downstream modules are compression and gRPC-Web; their module
interface does not own this proxy cache state. Upstream-module hooks are inherited
no-ops when enabled.

The fork request-cache entry checks enabled before generating a key and returns
before lookup while disabled; purge dispatch is skipped by false is_purge.
The response cache-task helper returns immediately when neither enabled nor
bypassing. ServeFromCache starts Off, and support_streaming_partial_write returns
None without the inner owner, so the ordinary disabled-cache state does not enter
cache-fill continuation solely because a downstream error occurs.

Edgion ResponseCache instead owns ctx.response_cache: request-tail bind/lookup,
private bounded capture and direct hit delivery in response_cache/lifecycle.rs.
It does not enable the fork field. Current ordinary Gateway HTTP therefore does
not make the inherited fork key/lookup/range/stale/purge implementations a cache
hot path to optimize. This does not mean Edgion has no HTTP response cache.

This is a current-source control-flow/ownership conclusion for ordinary Gateway
HTTP, not a blanket proof for fork examples, injected sessions, custom connectors
or future configurations. Re-evaluate on fork changes, new overrides, module/API
ownership changes or any cache enablement. Source anchors: fork Session::new,
HttpCache::new, proxy_cache request entry and cache_http_task; Gateway
pg_init_downstream_modules.rs, pg_request_filter.rs and response_cache/lifecycle.rs.
Seven existing own-cache lifecycle tests passed, but they do not execute every
inherited fork cache branch or establish custom-connector reachability.

## Inherited admission and custom-message call-site boundaries

Revalidated at `bcaa1b8ad` against fork `1aaa45cc70430bc782990bc20ddee3fef36d6b99`.
`process_request` calls `allow_spawning_subrequest` after successful early
filtering and before downstream request modules. Edgion's inherited false
avoids installing the application SubrequestSpawner in the new session. It
does not prohibit proxy-core background cache subrequests; those retain their
separate cache gates described above.

After request filtering, cache handling and subrequest cleanup, the ordinary
upstream path awaits the inherited `proxy_upstream_filter`, which returns true.
There is no Edgion rejection policy or additional header work in that default.
This source observation does not prove compiler elimination or allocation-free
async-trait dispatch, and adding an identical override is not an optimization.

The Gateway listener builder uses `ProxyServiceBuilder::new(...).server_options(...).build()`
without registering a custom connector/session handler. The fork builder defaults
to connector `()` and no handler; its build still calls internal `HttpProxy::new_custom`.
Gateway source has no Custom ALPN registration, and ordinary peer construction
selects HTTP versions. The fork upstream connector enters custom negotiation only
for Custom ALPN; the unit custom connector's methods are unreachable stubs, not
a transport fallback. This is a current ordinary-construction boundary, not a
guarantee for arbitrary externally injected peer/session values.

In both H1 and H2 proxy paths, custom downstream writers are taken only from
`downstream_session.as_custom_mut()`. Without such a writer, reader extraction
returns None and the custom-message injection channel/callback setup is skipped.
Ordinary HttpTask relay channels still exist; this audit does not claim a
channel-free HTTP path. The separate custom-upstream path has its own queues and
forwarding call. The inherited custom-forwarding method does nothing, and both
message filters return the supplied Bytes by move rather than copying its payload.

No new runtime optimization is selected for these defaults. Re-evaluate if
Gateway registers a custom connector, handler or ALPN, installs custom sessions,
overrides these hooks, enables application subrequests or updates the locked fork.
WebSocket upgrades and ordinary HTTP body processing are not themselves evidence
of Custom message-session reachability. Network transports, compiler-generated
future storage and other inherited callback call sites remain separate work.
This is source-level reachability evidence; no custom-transport or wire test ran.

## Direct response-head owner orchestration

Inspected at `a5c4a1d81`: pg_response_head.rs, the transcoder transport.rs,
Anthropic conversion head.rs and semantic Guardrail attached.rs. The dispatcher
prioritizes a transcoder plan, then a unary owner, then an active Guardrail claim.
The request-stage may_hold predicate is broader: possible unary work or any claim
can reserve the capability before activation. These predicates are not equivalent.

Transcoder head_plan rechecks valid response ownership, Origin source, status 200
and final JSON media rules. Streaming returns Immediate; unary bounds metadata
before retaining its header template and supplies the remaining absolute deadline
to Hold. Its boundary takes ready output only for a valid clean terminal decision;
other boundaries discard ready output and fail. Full reservation/output cleanup
has separate lifecycle owners and is not implied by that one discard.

Anthropic UnaryHead returns Immediate for an unsuccessful raw origin response.
Successful conversion revalidates final media/encoding, method/status eligibility,
source and active competing claims. Its first deadline publication retains an
absolute instant and source: request wins ties over gRPC, then the Hold ceiling.
Fresh compact metadata projections intentionally release original capacity;
removing copies merely because HeaderValue can share backing storage would change
that retained-memory boundary. The error template and later error reply also have
independent ownership.

Guardrail checks final SSE eligibility, 2xx status and canonical all-field identity
encoding before Hold. Its template retains the first Server/HSTS values, fixed
replacement metadata and canonical managed response IDs. OnceLock keeps the first
successful publication, but prepare_head_replacement still builds each candidate
before set; unary error-template preparation has the same construction/publication
distinction. The bounded lifecycle audit below constrains successful Hold but
does not establish unrestricted callback multiplicity. Replacement clones
and frozen state cannot be replaced with a destructive take without ownership
review; [guardrail-01](guardrail-01-held-replacement-header-storage.md) already owns
that decision.

No new behavior-preserving change was selected. In particular, replacing decimal
String insertion with numeric HeaderValue construction is not automatically an
allocation saving: locked http 1.5.0 From<usize> reserves maximum decimal width in
BytesMut. Existing [cache-02](cache-02-age-header-construction.md) measurements are
for its recorded u64 case, not a new usize or Guardrail benchmark.

The direct will-commit callback excludes 101, claims the final writer and seals
cache headers. The fork response_pipeline selects the plan after response_filter
while awaiting the final head. This source observation does not establish every
barrier transition, repeated callback, transport commit or cancellation schedule.
Focused checks passed eight response-head, seven unary-head, two attached-Guardrail,
ten transcoder-transport and sixteen transcoder-response tests (43 total).

## Fork Hold lifecycle and retry boundary

Inspected at `7bab9c423` against the locked fork above. ResponseHeadBarrier::select
rejects selection outside AwaitingFinalHead. The pipeline calls the plan only for
a final header (including 101) while awaiting selection. H1 and H2 create a fresh
ResponsePipelineState per pump invocation; neither fact alone proves one plan
invocation per request across failures and attempts.

After successful Hold selection and successful release-latch arming, the pipeline
sets the Session's response_head_attempt_selected flag. This flag is monotonic
for that Session. response_head_retry_closed checks it or an independently
committed final response. The fork clears retry before the product
error_while_proxy hook and again in the outer attempt loop. Gateway's corresponding
hook retains its earlier failure accounting, then checks this closure before AI
scheduling or retry-budget decisions. Thus an armed Hold cannot start another
ordinary retry through this loop merely because the retained head is not written.

This excludes a plan error, failed barrier selection or failed latch arming before
the marker. Immediate does not set this marker at plan selection: the output path
calls response_head_will_commit, marks writer handoff only after that succeeds,
then awaits prepare_response_headers. Selection is not proof of transport write
completion. Do not generalize these observations into an unconditional
once-per-request callback contract or remove owner-level first-publication guards.

Both H1/H2 call cancel_response_head_hold after the joined pumps return. Its
claim_boundary moves Holding to Aborted and drops retained tasks; when already
resolved/aborted it returns None, so repeat cancellation does not publish another
Cancelled outcome. Release/Replace usage is separately consumed with Option::take.
These are explicit-return cleanup observations, not proof of notification when
an enclosing future is dropped, nor complete transport-interleaving coverage.

No new template-construction guard was selected solely to prevent repetition
after successfully armed Hold: this path already closes ordinary retries. The
separate Guardrail representation decision remains open. Eight Gateway head tests
and eleven proxy-error tests passed in this audit; they do not execute the fork's
entire lifecycle matrix. The fork held_head_and_body_cross_batches_until_release
test was inspected (one plan call and retry closed while no header is prepared),
not executed. Formatting, workspace check/Clippy and document validation passed.

## Direct trailer dispatch and terminal transfer

Inspected at `2bf6b8570` against the locked fork. Gateway's upstream trailer hook
dispatches request-owned companions before unary Anthropic trailer compaction.
The downstream hook returns None without a transcoder; with one it clears the
trailers and transfers terminal Bytes through lock().take(), without cloning
the payload. The existing [typed status evidence decision](grpc-08-typed-terminal-status-evidence.md)
owns the original-status snapshot alternative. INDEX's trailer-policy and bounded
unary metadata audits own their snapshot and fresh-storage constraints; copying
there is not automatically redundant. No duplicate optimization is proposed.

In the fork's normal successful source path, a claimed terminal-body event is
processed before the present upstream trailer callback. Errors/deadlines can
return earlier. The downstream ordered-task helper invokes the trailer callback
only for Some trailers: returned Bytes become a terminal Body task; otherwise
the existing trailer owner moves through mem::take and normalization. Downstream
body filtering then observes the resulting task. Module-level trailer-to-body
translation in Session is a separate stage, including gRPC-Web behavior.

Revalidated at `85973e45f`: the inherited downstream response_body_filter is a
synchronous function, unlike the asynchronous upstream event hook. The fork
helper invokes it directly and sleeps only for a returned Some(Duration); this
Gateway default returns None. There is no per-hook async future or requested
sleep to remove by adding an equivalent override. The surrounding task helper
remains async, and this observation makes no generated-code allocation or
whole-pipeline scheduling claim. Existing terminal and module ordering stays as
recorded above. This clarification used source inspection, not new wire tests.

The locked fork already owns a separate downstream terminal latch and rejects
nonempty output from a synthetic terminal observation. The
[terminal-dispatch knowledge](../../skills/04-review/h2-grpc/regression-guards/trailer-done-terminal-body-dispatch.md)
now distinguishes that current source fact from its older nonadoption snapshot.
This audit passed eleven upstream-trailer, six callback-cancellation and seven
unary-head Gateway tests, formatting, workspace check/Clippy and doc validation.
Fork regressions were not executed. Concrete processor internals, all module
transforms and full transport/interleaving coverage are not closed by this audit.

## Concrete remaining checks

- Fork successful Hold release currently transfers its suffix through a temporary
  Vec. The dependency-owned allocation candidate and required ownership tests are
  deferred in [response-02](response-02-fork-held-task-transfer.md); no fork change
  or complete barrier-lifecycle verification is claimed.

- Response-head ownership: direct owner orchestration, successful Hold retry
  closure, explicit pump-return cancellation, selection-failure state transitions
  and field destruction are inspected above and below. Full callback multiplicity,
  deeper response execution and transport/cancellation interleavings remain
  separate work; field destruction is not callback-notification evidence.
- Inherited warning hooks: eleven retry/cache call sites and log/filter ordering
  are inspected in [the ownership decision](log-07-fork-warning-ownership.md).
  Diagnostic equivalence and cache/custom reachability remain unproven. The false
  default is not policy approval or permission to suppress useful errors.
- Fork cache defaults: ordinary HTTP enablement and entry guards are inspected
  above. Reopen this boundary when a cache owner, override or dependency changes;
  do not expand the conclusion to uninspected custom/injected sessions. Edgion
  ResponseCache has separate [replay ownership](INDEX.md#response-cache-replay-body-ownership-decision)
  and [canonical LRU lookup](INDEX.md#canonical-lru-routing-and-map-hashing-decision)
  evidence. Those scoped audits do not establish complete cache coverage or
  resolve the deferred ownership and hashing decisions.
- Continue the stage-specific open dependencies in INDEX. The dispatcher table
  supplies entry points, not replacement evidence for those reviews. Other
  transport inventories remain governed by their existing coverage records.

## Verification limits

Eight response-head tests and one request-summary test passed on this baseline;
formatting and workspace all-target check/Clippy passed with unrelated warnings.
These are focused behavior checks, not exhaustive callback coverage. No runtime
or tests changed. No benchmark, full suite or live integration was performed.
Independent plan and final reviews passed; source-to-table method sets match
exactly, and agent-doc validation and diff checks passed.


## Selection failure and destruction boundaries

Source inspection at `0eaaeb889` against the same locked fork narrows the earlier
preselection-failure entry without claiming complete cancellation verification.

- `response_head_commit_plan` errors propagate before `head_barrier.select`.
  This says nothing about application-side work performed before returning Err.
- `ResponseHeadBarrier::select` rejects a non-Awaiting state before mutation.
  For Hold, the monotonic deadline is checked before the Holding assignment and
  Box allocation. Deadline overflow leaves the prior Awaiting state intact.
- Release-latch arming follows Holding selection. Arming rejects a non-Disarmed
  sink without changing its control/work state. That failure leaves the barrier
  Holding and returns before this site's Session marker assignment. It does not
  prove the Session marker was previously false or that retry is then permitted.
- Unsupported-mode and initial work-limit handling occur after successful latch
  arming and marker assignment, through the existing typed boundary path.

Neither ResponsePipelineState nor ResponseHeadBarrier defines a Drop hook that
calls application response-head callbacks. Dropping the pipeline drops its
Holding Box, task Vec and owned HttpTask fields. This releases those owners;
shared Bytes or other referenced allocations can remain alive through other
owners. It is not proof of a Cancelled outcome notification, log submission,
transport completion or release of every application-owned reservation.

Explicit `cancel_response_head_hold` instead claims a Holding boundary, disarms
the sink, forbids upstream reuse and calls the Cancelled outcome hook. Other
states return without a second notification. Gateway body/trailer execution
leases separately release inflight processor work when an incomplete callback
future is dropped; they do not establish a general response-head Drop callback.
Keep these ownership boundaries separate when considering destructive takes,
first-publication guards or cleanup shortcuts.

No runtime optimization or defect was selected in this audit. The concrete
selection states above are inspected; full attempt multiplicity, owner-specific
reservation cleanup and transport interleavings remain outside this source-only
result. Fresh formatting, workspace check/Clippy and document validation passed
with existing unrelated warnings. No Gateway/fork tests or benchmarks were run.

## Terminal logging owner inventory reconciled

Revalidated at `6c943c23d`. This closes the inventory of direct terminal cleanup
calls in pg_logging, not the entire reachable implementation graph or the error/
retry stage. Earlier broad references to "remaining finalizers" should use these
concrete boundaries instead of restarting the same ownership inspection.

| Direct owner / handoff | Current ownership and review disposition |
| --- | --- |
| Active-context gauge | Release guard is acquired before awaits; retain cancellation/unwind balancing. |
| Wasm | Context takes the request state; finish drains bindings and closes guest contexts. Abort/drop does not re-enter the guest. Existing Wasm lifecycle/lease audits remain applicable. |
| ResponseCache | Pending publication moves out before awaited finish_body; only accepted completion publishes. Preserve deadlines, epoch/freshness fences and cancellation ownership. See INDEX's response-cache terminal ownership audit. |
| Transcoder | Ordinary processors finalize before plan logging and output/reservation cleanup. The temporary plan Arc supports mutable-context access; [grpc-05](grpc-05-terminal-log-owner-borrow.md) owns that decision. |
| Backend ledger and metrics | Final attempt commit is consumed with take; byte counters and request metrics remain required without log sinks. Existing terminal metric-handle audit covers the direct dispatch. |
| AI accounting | Driver/builder fallback precedes attempt, usage and quota settlement. Summary publication and frozen accounting snapshots retain their existing ownership; [usage-02](usage-02-accounting-snapshot-reuse.md) remains the decision owner. |
| Response terminal state | Claimed/open state selects scalar settlement/close; no new owned payload is created here. |
| Request handlers and observers | Handler collection moves out and back while logs transfer; observers finalize and take their logs. The first production observer sweep is not an empty repeated collection; see below. |
| Semantic/trailer processors | Driver paths retain their mutex and cancellation gates; pre-driver fallback and trailer cleanup claim remain necessary. [body-06](body-06-empty-terminal-role-locks.md) owns optional-role lock elision. |
| Attached ExtProc/Guardrail | Summaries enrich retained stage logs. Keep [log-05](log-05-attached-summary-rendering.md) and [extproc-01](extproc-01-terminal-stream-retirement.md); no duplicate proposal. |
| WAF | Current direct finalizer changes Running/Detected to Incomplete when the logging phase is absent; it does not copy the findings collection. |
| Mirror | Both explicit no-consumer and sending paths take/drop the handle. A dropped JoinHandle detaches; cancellation during the send await does not prove that later explicit cleanup executed. |

The observer collector reserves self.len slots even on a repeated empty sweep.
However, current production references form one chain: pg_logging calls
drain_request_body_observer_logs, which calls finalize_incomplete_and_take_logs.
Every installed observer starts with Some(log); Complete/Abandoned events finalize
the outcome without taking the log. Repeated empty-sweep tests therefore do not
demonstrate an allocation to eliminate on the normal production collection path.
No optimization or deferred task was selected for this hypothetical repetition.
Reopen if another production drain or changed slot-log lifecycle is introduced.

The locked fork has three logging call sites: finish, the local-response return
branch after request_filter, and the error-handler terminal path. Do not infer
"only from finish" from the current Gateway comment. These terminal paths do not
by themselves demonstrate repeated collection for one request, nor do they prove
cleanup notification when an enclosing future is dropped before logging.

Fresh existing pg_logging tests passed 11 cases, including the subprocess matrix
for disabled sinks, cleanup and test-store behavior. Formatting, workspace check/
Clippy and agent-doc checks passed with existing warnings. No new source/test
behavior, allocation measurement, full suite or live transport test was added.
Concrete processor execution, retry decisions and external transport internals
remain their own bounded review items; this inventory is not a blanket closure.

## Retry adapter and bounded AI scheduler reconciliation

Revalidated at `48ff77c48`: pg_fail_to_connect, pg_error_while_proxy,
ai/http/attempt.rs retry adapters, ai/runtime/dispatch.rs outcome/skip handlers,
and ai/runtime/attempt.rs. The direct decision and transition bodies are reviewed;
this does not close asynchronous admission or settlement execution.

- Ordinary connect failures retain per-attempt status/error/outlier accounting,
  then resolve request/gRPC deadline ownership before retry. The AI branch
  delegates endpoint-scoped predispatch release/advancement; ordinary connect
  retry checks relay and attempts before its optional retryConstraint gate.
- Proxy failures retain the local-reply exception and transport accounting.
  A closed response head vetoes retry before AI scheduling. Ordinary per-try
  timeouts, method-safe reuse, ambiguous H2 outcomes and Pingora-decided retry
  ceilings remain distinct branches. Ordinary reuse does not consume the
  retryConstraint gate; do not describe all branches as sharing one budget path.
- AI raw-response processing owns status classification, cooldown observation,
  scheduler transition and retry-budget decision. The later status-retry error
  adapter clears retry_on_status and marks Pingora retry without repeating that
  transition or budget consumption. It does not freshly redo every deadline.
- Other classified AI transport failures use live replay/deadline/commit facts
  before dispatch outcome handling. Excluded failures return without those
  unused checks, as already implemented. Predispatch credential/backend skips
  do not consume a dispatch slot or failure-ledger entry.
- AttemptScheduler stores at most 16 backends, 16 credentials per backend and
  eight dispatched failures in fixed arrays/bitmaps. Its transitions use scalar
  arithmetic, no heap collections, locks, I/O or timers. Credential selection
  scans at most 16 weights; exclusions and caller-supplied current eligibility
  affect both total weight and ticket resolution. No cached-weight replacement
  was selected. These properties do not apply to all surrounding adapters.

Existing owned admission projections pin the selected generation across awaits;
deadline checks before preparation and after admission/resolution remain live
checks, not redundant compilation candidates. Error context and attempt logs
remain request-owned. The earlier candidate-buffer reuse and borrowed retry-budget
changes already cover their recorded costs; no new runtime change or deferred
task was selected in this reconciliation.

The concrete asynchronous seams left outside this result are
AiAttemptRuntimeState::admit/drain_pending and the endpoint/transport operations they
surround; their generation, quota and cancellation execution belongs with the
upstream-selection and accounting reviews. Do not reopen the pure scheduler or
direct retry adapters merely because those dependency seams remain open.
The direct admit/drain ownership seam is subsequently reconciled below; the
exclusion here describes the retry-adapter audit's scope.

Fresh tests passed 72 cases: HTTP AI attempt 7, pure scheduler 9, dispatch 36,
connect failure 9 and proxy error 11. Formatting, workspace check/Clippy and
agent-doc validation passed with existing warnings. No source/test changes,
live provider/Redis, full suite or performance measurement ran.

## AI admission and settlement ownership reconciled

Revalidated at `ecbfcc430`: AiAttemptRuntimeState::admit, finish_predispatch,
finish_failure, finalize, settlement_for, push and drain_pending, together with
the local reserve/settle helpers in ai/runtime/attempt_runtime.rs. This completes
the named direct owner-orchestration check left by the retry audit above.

Admission acquires the logical breaker permit, then the outlier/probe owner,
before optional Redis reservation. Early exits and cancelled futures release
local RAII owners; they do not prove a remote reservation was refunded. Owned
input remains across reserve's await and success moves its fields into the
runtime. The previously implemented limiter borrow already avoids its extra Arc
clone on rejected/failed admissions; only a successful lease needs that owner.
Bounded identity compaction preserves retained capacity, not just text length.

The reservation lease pins its Redis client, limiter, keys, limits and original
TTL. Re-resolving the client or regenerating those inputs during settlement would
change generation/window ownership. Pending items move from the bounded queue
into settle without cloning. Release and Reconcile execute serially under the
existing 500 ms operation timeout; Retain issues no Redis command. A popped item
is not requeued if its await is cancelled. Missing reservations do not enter the
queue; overflow past eight entries drops the lease and preserves the remote
charge's TTL fallback instead of inventing a refund or detached retry.

Predispatch release, known usage reconciliation, ambiguous transport retention
and unavailable-accounting retention remain distinct. A deliberate gateway cut
can reconcile valid usage without claiming upstream EOS or outlier success.
Local cleanup, remote timeout and exactly-once execution are not interchangeable.
Skipping queued Retain owners would alter release timing; no measured need or
new task was selected for that representation change. Existing
[snapshot reuse](usage-02-accounting-snapshot-reuse.md) and
[attempt size accounting](log-02-incremental-attempt-size.md) decisions remain.

Fresh tests passed 25 attempt-runtime and 32 shared Redis-core cases (57 total),
plus formatting, workspace check/Clippy and agent-doc validation with existing
warnings. The tests exercise local policies and conversions; they do not prove
live Redis cancellation, failover or wire execution. No runtime changes, new
tests, live backend experiment, full suite or performance measurement ran.
The direct owner seam is reviewed; shared Redis transport execution and concrete
response-processor work remain separately scoped rather than reopening this seam.

## Concrete response processor direct-entry inventory

Reconciled at `6b76ed5ac`. The current Gateway source contains nine
`StreamingResponseProcessor` implementations. This inventory covers their direct
trait entry/control flow and ownership handoffs; it does not claim every reachable
parser, runtime, codec or transport dependency is complete. These are implementations,
not nine independently installed slots. Read-only body-filter chains and separate
trailer handlers are outside this enumeration.

Paths below are relative to `edgion-gateway/src/`.

| Implementation / source | Direct entry and owner boundary |
| --- | --- |
| `AiAttemptResponseProcessor`, `ai/runtime/dispatch.rs` | Captures raw encoding eligibility before child header normalization; forwards headers, typed body events and cleanup to existing children. Stops child body dispatch after sink termination and records the event-derived EOS latch. It does not override/forward the real-trailer hook. The local usage owner and native Messages quota handoff have already been moved without removing shared owners. |
| `UsageExtractionProcessor`, `ai/accounting/usage.rs` | Borrows non-SSE input; retaining SSE decode shares raw handles. Publishes canonical batches for active guarding, including explicit EOS, and reclaims current raw bytes when its terminal decision preempts the guard. Encoding diagnostics and quota gates remain distinct. Chat and Responses share the bounded selective JSON owner; their protocol completion policies differ. |
| `MessagesProcessor`, `ai/protocols/anthropic/processor.rs` | Shares input with the retaining SSE decoder or feeds the bounded JSON observer by reference. Preserves native bytes and reported-only accounting. Actual-model ownership moves from the selected start observation; unknown events cannot supply lifecycle evidence. Incomplete finalization marks unavailable; Reject termination retains native error ownership. |
| `AnthropicResponseProcessor`, `ai/conversion/chat_to_anthropic/response.rs` | Header-selected fixed/unary/streaming modes remain explicit. Streaming canonical metadata is conditional; terminal reclamation exposes translated client bytes only. Release/preemption drops unary storage/owner and seals the processor. The bounded unary exception is separate from streaming conversion and ordinary HTTP. |
| `SemanticSseSource`, `ai/protocols/openai/canonical_sse.rs` | Active SSE guarding takes input into the bounded decoder/batch, preserves typed control/poison classification and publishes EOS even without data. Encoded/degraded sources become unavailable; overflow/publication failure arbitrates terminal ownership. Default cleanup hooks perform no extra work. |
| `GuardrailResponseProcessor`, `ai/guards/semantic/response.rs` | Consumes canonical batches through RawWindow retain/decision/resolve generations. Awaited decisions, rollback output and held-head policy keep separate bounds. Preemption/cancellation discards its window and disables the owner; aggregate summaries publish through the attached owner. |
| `ExtProcResponseProcessor`, `plugins/http/ext_proc/response_streamed.rs` | Explicit typed terminal handling leaves completion to a configured real trailer exchange; otherwise sends a final bounded body exchange. Keeps original input across the await for failure policy while protobuf owns its copy. Valid replacement Vec moves into Bytes. Common verdict validation precedes mutation; cancellation delegates release to the attached owner. |
| `WasmResponseObserver`, `plugins/http/wasm/attached.rs` | Both body entries call synchronous observe directly. A binding execution loan installs a shared body handle in a read-only guest frame; guest failure disables the binding rather than replacing the stream. Trailer-aware EOS uses closes_without_trailers. Release/preemption aborts the binding. |
| `GrpcResponseProcessor`, `plugins/http/grpc_json_transcoder/response.rs` | Takes input into bounded framing/conversion; output owns admission credit. Only this implementation overrides the actual-upstream-trailer hook, which settles from real status evidence. Chunk EOS does not fabricate success. Release/finalization/preemption share local cleanup while separately held output retains its own lifetime. |

Five implementations use the default typed-event bridge to legacy on_chunk:
Usage, Messages, Anthropic conversion, SemanticSseSource and Guardrail. AiAttempt,
ExtProc, Wasm and gRPC override on_event. The existing typed-forwarding audit and
profile-before-public-gate decision still apply; this inventory asserts no
zero-allocation callback. Semantic flush/EOS, real gRPC status, local release and
successful downstream delivery are different facts.

Existing decisions remain the owners of disputed changes: Guardrail
[header storage](guardrail-01-held-replacement-header-storage.md),
[snapshot encoding](guardrail-02-decision-snapshot-encoding.md),
[callback output](guardrail-03-callback-output-capacity.md) and
[window statistics](sse-01-incremental-window-statistics.md); ExtProc
[protobuf storage](body-02-extproc-protobuf-body-storage.md) and
[terminal retirement](extproc-01-terminal-stream-retirement.md); gRPC
[wire schema](grpc-03-prepared-wire-schema.md) and
[output ownership](grpc-04-single-record-output.md); and the existing Wasm
representation/lock decisions. No duplicate proposal or new runtime edit was selected.

Remaining deeper investigation includes canonical tokenizer/semantic-window
operations beyond their recorded audits, converter parsing/serialization beyond
existing scoped records, Wasmtime/guest ABI internals, gRPC reflection/wire codecs
and outbound transport execution. This replaces an unspecified processor-entry
backlog; it does not close those dependency graphs or the overall campaign.

Fresh existing tests passed 52 usage, 36 dispatch, 25 Guardrail response,
12 canonical SSE, 32 converter response, 13 native Messages, 11 ExtProc response,
5 Wasm attached and 16 transcoder response cases (202 distinct tests). The first
parallel Wasm selection passed four and failed its timer test with preparation
capacity exhausted; all five passed unchanged with --test-threads=1. The process
preparation semaphore defaults to four permits. Serial success does not establish
parallel isolation. No new tests, live transport or benchmark ran.

## Read-only response body filter direct-entry inventory

Reconciled at `05c842fee`. Four current Gateway implementations implement
`UpstreamResponseBodyFilter`; they are separate from the nine writable streaming
processors above. Read-only describes the body argument, not session immutability:
entry dye installation and IP-denial state still mutate the request context.

| Implementation | Direct work and retained boundary |
| --- | --- |
| `ConditionalUpstreamResponseBodyFilter` in `plugins/runtime/conditional_filter.rs` | Uses compiled conditions and their logless evaluation before invoking its inner filter. Descriptors, regexes, time bounds and dependency bindings are prepared; current time, probability, context and dependency results remain live. A skipped condition returns no delay. |
| `ExtensionRefFilter` in `plugins/runtime/gateway_api_filters/extension_ref.rs` | Resolves the request-pinned runtime silently, including backend placement restrictions. Checks IP dependencies, installs each nested entry's dye plan, invokes filters in order and takes the maximum delay. Stops nested execution on IP denial. The runtime Arc retains ownership during mutable session dispatch; this is not a disposable payload copy. |
| `BandwidthLimit` in `plugins/http/bandwidth_limit/plugin.rs` | Uses the construction-time parsed f64 rate and current nonempty chunk length to compute a Duration. Zero rate/empty input returns none. The accepted per-response, stateless delay contract has no shared token bucket or elapsed-time correction. |
| `WasmPlugin` in `plugins/http/wasm/plugin.rs` | Resolves/schedules module refresh, skips absent body exports, acquires a bounded lease and installs the current shared Bytes handle in an otherwise empty body frame. Clears that handle before lease return. Pause uses the existing bounded metric; acquire/trap failure leaves streaming intact. This standalone stage differs from the request-attached observer's execution lifetime. |

`pg_upstream_response_body_filter` dispatches route (gRPC preferred), selected
backend (gRPC preferred), Gateway and global runtimes in that order. Runtime and
ExtensionRef loops merge delays by maximum, not sum, and stop invoking later
filters after IP denial. Writable processor sink termination alone does not skip
the read-only chain for the terminating chunk. Runtime IP dependency checks occur
before the empty-filter shortcut; do not infer that an empty list makes the
runtime irrelevant.

Typed `TerminalWithoutTrailers` runs processors and the read-only chain with EOS.
`TerminalBeforeTrailers` runs the full read-only chain with EOS false only when
processors flush nonempty bytes; otherwise it still checks IP dependencies.
Actual trailers retain their separate completion ownership. These distinctions
prevent replacing terminal dispatch with a generic empty-chunk shortcut.

No new safe optimization was selected. The existing
[prepared dispatch](body-01-prebuild-readonly-dispatch.md),
[IP projection](ip-01-session-address-projection.md),
[JWT condition ownership](condition-01-jwt-claim-evaluation.md) and Wasm decisions
remain the proposal owners. In particular, logless evaluation does not prove
zero allocations: live JWT JSON parsing and eager candidate storage remain an
unresolved conflict with the body-phase allocation rule, not an accepted exception.
This inventory closes the direct-wrapper enumeration, not deeper matcher/parser,
Wasmtime, dependency-publication or network execution coverage.

Fresh tests passed 18 conditional-wrapper, 70 condition, four bandwidth,
11 ExtensionRef, 31 response-body callback and 44 standalone Wasm cases (178 total).
Wasm ran serially because process admission is shared. No new tests, full suite,
live transport or benchmark ran.

## Inbound PROXY policy callback inventory

Revalidated at `4c19f3a93` against the locked fork above. This supplements the
HTTP inventory with two connection callbacks in
`runtime/server/proxy_protocol_policy.rs`; it does not close the whole listener
or fork parser graph.

| Callback | Direct work and ownership |
| --- | --- |
| `GuardFirstConnectionFilter::should_accept` | One Acquire load of the monotonic fatal flag; reject immediately when set, otherwise borrow the optional inner filter across its await, or allow when absent. No state-mutex acquisition or Arc clone in this body. |
| `CidrProxyProtocolTrust::is_trusted` | Pass the physical transport peer IP to the prepared canonical `IpRadixMatcher::contains`; no CIDR parsing or matcher construction in this callback. |

`preflight_and_freeze` prepares CIDR matchers before freezing the initial socket
policy; publication/diagnostic mutexes are outside these callback bodies.
`listener_builder::apply_connection_filter` installs the composite for redirect
HTTP, ordinary HTTP/HTTPS, TCP, TLS termination-to-TCP and TLS passthrough. Its
current builder always supplies `Some(StreamPluginConnectionFilter)`, despite
the composite's optional type. UDP admission is a separate path.

The fork's `listeners/l4.rs::ListenerEndpoint::accept` invokes the filter when
both socket digest and peer address are available; missing digest/peer bypasses
that call. The filter precedes `UninitializedStream::handshake`, inbound PROXY
processing and TLS. The guard samples fatal state before awaiting the inner
filter and does not recheck afterward; this is not an in-flight revocation claim.

In `protocols/l4/proxy_protocol.rs::apply`, TrustedSources checks the transport
`peer_addr()` before any address override and before header probing. Untrusted
peers skip parsing; trusted peers without a recognizable header remain ordinary
connections. Missing identifiable peers fail conditional trust. Required mode
has no trust object and requires parsing; Disabled skips PROXY processing.
These distinctions must survive any future dispatch optimization.

No runtime change was selected. Async-trait boxing and the fork's per-stream
trust Arc ownership prevent a whole-accept zero-allocation claim. The existing
[stream dispatch decision](stream-01-synchronous-plugin-dispatch.md) already
requires separating outer filter costs from per-plugin dispatch before changing
representation. No duplicate proposal or measured performance benefit is claimed.

## Synchronous response-header filter direct-entry inventory

Revalidated at `9dd0930aa`. There are nine concrete implementations of
`UpstreamResponseFilter` under Gateway source. This is the synchronous final-header
stage, distinct from async `UpstreamResponse` and read-only body filters. Source
paths below are relative to `edgion-gateway/src/`.

| Implementation / source | Direct work and ownership boundary |
| --- | --- |
| ConditionalUpstreamResponseFilter (`plugins/runtime/conditional_filter.rs`) | Evaluate compiled conditions against live session inputs, retain skip cause in the stage log, otherwise delegate; alias/dye and log suppression delegate through the wrapper. |
| DslPlugin (`plugins/http/dsl/plugin.rs`) | Drive execute_safe through the intentional multithread Tokio block_in_place / Handle::block_on bridge. The synchronous trait does not make DSL outbound calls nonblocking; VM and bounded outbound policies remain authoritative. |
| WafPlugin (`plugins/http/waf/mod.rs`) | Claim phase, take the transaction, borrow current header bytes into temporary descriptors and publish transaction-owned state; execute/record outcome, intervene or restore the transaction. Dedicated WAF observability suppresses generic stage logging. |
| ResponseRewrite (`plugins/http/response_rewrite/plugin.rs`) | Prepared status/log and rename-protection facts; dynamic rename values, then prepared add/set values and removals. Preserve rename/add/set/remove order, duplicates and partial-success diagnostics. |
| DebugAccessLogToHeaderFilter (`plugins/runtime/gateway_api_filters/debug_access_log.rs`) | Wrap suppression precedes context access; otherwise borrow access-log projection, serialize owned JSON and invoke the response setter, ignoring its result. Accepted development-surface size/failure limitations remain. |
| GapiCorsFilter (`plugins/runtime/gateway_api_filters/cors_filter.rs`) | Read live Origin, check allowed origin, build actual-response CORS headers, set them and append Vary: Origin. Session mutation and reflection ownership remain separate from prepared matching. |
| ExtensionRefFilter (`plugins/runtime/gateway_api_filters/extension_ref.rs`) | Resolve the pinned runtime, enforce target/backend-scope restrictions and IP dependencies, create nested logging ownership, install each dye plan and dispatch in order until a non-continuing result. Runtime handles outlive mutable session calls. |
| ResponseHeaderModifierFilter (`plugins/runtime/gateway_api_filters/response_header_modifier.rs`) | Execute the cold validated HeaderModifierPlan in set/add/remove order; prepared HeaderValue clones transfer owned values through guarded session writers. |
| WasmPlugin (`plugins/http/wasm/plugin.rs`) | Nonblocking module lookup with preload/fail-mode on miss, bounded lease acquisition, request/response header and property projection into guest phase state, response callback and finish_phase result handling. No zero-copy guest projection claim. |

`pg_upstream_response_filter` skips interim informational headers except terminal
101 and performs origin arbitration before the chain. Its shared sync tail replays
pending response operations without consuming them, then runs gRPC-preferred route
and backend runtimes, Gateway runtimes and global runtimes. Later layers stop on
response_stage_settled. Empty-entry count gates belong to Gateway/global loops;
route/backend runners still enter runtime IP-dependency checks. Processor framing
is repaired when unsettled and the late request-body view is released afterward.
`cached_response_filter` normalizes and selects the cache-hit response, then enters
the same tail without origin timing, retry, health or persistence work.

`PluginRuntime::run_upstream_response_plugins_sync` checks IP dependencies before
entries, installs dye plans, honors dedicated-log suppression, times ordinary
entries and converts termination results through the shared response-stage helper.
The log vector reserves only on its first retained entry. ExtensionRef retains
nested resource-log ownership rather than flattening it into the outer entry.

No new runtime edit was selected. Existing decisions remain independent:
[condition ownership](condition-01-jwt-claim-evaluation.md),
[rename fidelity](response-02-rename-value-fidelity.md),
[CORS value transfer](cors-02-session-header-value-transfer.md),
[stage log capacity](log-06-stage-log-capacity-policy.md) and
[Wasm map storage](wasm-02-map-decoding-profile-storage.md).
The original debug-surface and response-header mutation knowledge still govern
masking, protected fields and failure behavior. This inventory does not close the
async header entry inventory, deep VM/WAF/guest execution, session mutation,
serialization or complete transport behavior.

## Asynchronous response-header companion direct-entry inventory

Revalidated at `613496476`: seven concrete `UpstreamResponse` implementations.
An async trait permits waiting; it does not mean every implementation awaits or
owns a full response. Paths below are relative to `edgion-gateway/src/`.

| Implementation / source | Direct work and ownership boundary |
| --- | --- |
| ConditionalUpstreamResponse (`plugins/runtime/conditional_filter.rs`) | Await detailed live condition evaluation; errors return their PluginRunningResult immediately. Skip records an optional cause and returns Nothing; otherwise await the inner entry. Alias/dye and suppression ownership remain with the wrapper/inner contract. |
| BodyGatedUpstreamResponse (`plugins/runtime/body_gate.rs`) | Body-condition Skip bypasses the read and invokes the inner entry. Otherwise request a capped response-body read, enforce the prepared logical limit and failure policy, then invoke inner on success. This wrapper is not authority for full-response aggregation. |
| Guardrail (`ai/guards/semantic/plugin.rs`) | Find the attached request owner; require a 2xx SSE identity-encoded canonical AI response; install a bounded semantic processor or apply precommit failure. Client, permit and owner Arcs belong to the subsequent processor; the local owner remains available for installation failure. Generic stage logging is suppressed. |
| Arc<WasmPlugin> (`plugins/http/wasm/attached.rs`) | Borrow the request binding through an execution loan, gate duplicate headers, supply actual EOS to an enabled guest header hook, handle unsupported pause/local response, then install configured body/trailer companions. Missing binding/loan does not create a new context; shared bindings and loan cleanup own later callbacks. |
| ExtProc (`plugins/http/ext_proc/plugin.rs`) | Require a nonterminal attached owner, resolve effective modes and actual header EOS. Either execute the no-send terminal/companion plan or await a protobuf header exchange, validate verdict/mode changes, apply mutations and install or finish owners. Immediate-response and failure policy remain authoritative; owned protocol data crosses the awaited exchange. |
| WafPlugin (`plugins/http/waf/mod.rs`) | Claim ResponseComplete, take the transaction, execute an empty EOS ResponseWindow and logging phase, record dedicated outcomes and select intervention. This runs in the header stage and is not evidence that actual transport body completion occurred. It does not aggregate the upstream body. |
| ExtensionRefFilter (`plugins/runtime/gateway_api_filters/extension_ref.rs`) | Resolve silently for the empty-async-stage gate; active bundles enter normal pinned-runtime validation, nested log ownership and IP checks, then install dye plans and await entries in order. Return the first non-continuing result. Missing/unsupported targets produce a response-stage 500 result. |

The concrete response-body read adapter in `plugins/runtime/session_adapter.rs`
returns empty bytes only for its agreed bodyless disposition, returns refusal
where required, and returns Unimplemented for Buffer. Its immutable/live method
and status checks remain required. Neither the wrapper nor its mock tests establish
a supported complete-response read; DSL RespBody validation and the canonical
streaming/body ownership invariants still apply.

`pg_response_filter` sanitizes interim informational responses and returns before
plugin dispatch; terminal 101 remains distinct. For ordinary final responses it
sanitizes before plugins when unsettled, applies server headers, and runs the
ordered gRPC-preferred route/backend, Gateway and global async layers. Settled
state prevents later chains but still reaches pending local-reply delivery or
undeliverable-denial handling. Gateway/global empty-chain gates do not describe
the route/backend runners. After plugins, unsettled responses receive processor
framing repair and non-101 sanitizer repair before final commit handling.

The async runtime owns its session adapter across awaited entry execution, checks
IP dependencies first, installs dye plans, honors dedicated-log suppression and
applies shared termination conversion before collecting ordinary stage logs. Selected runtime
owners and attached session/processor handles cannot be replaced with short
borrows solely because their direct callback bodies look small.

No new runtime change was selected. Existing condition, ExtProc retirement, Wasm
representation and Guardrail capacity decisions remain separate. Together with the
synchronous and read-only body inventories, this reconciles current direct
response-stage plugin entries, not their complete nested condition, protocol,
VM, transaction, codec or transport dependency graphs.

## Request-stage orchestration direct-entry inventory

Revalidated at `3d231f9df`: source search across `edgion-gateway/src` finds 60
explicit RequestFilter implementations, 57 under plugins and three under ai
(AiProxy, AiGuard and Guardrail). This table covers the ten runtime orchestration
entries only. The other 50 implementation entries are outside this table; later
inventories below reconcile subsets. They are not 50 unique or wholly unreviewed plugins. In particular,
WasmPlugin and Arc<WasmPlugin> are separate entries. Existing scoped findings and
verification remain useful without establishing complete transitive coverage.
Paths below are relative to `edgion-gateway/src/plugins/runtime/`.

| Implementation / source | Direct work and ownership boundary |
| --- | --- |
| ConditionalRequestFilter (`conditional_filter.rs`) | Own alias/dye metadata and compiled optional conditions; evaluate live conditions asynchronously, return errors or a logged Skip, otherwise await inner. Absence of conditions does not imply absence of metadata. |
| BodyGatedRequestFilter (`body_gate.rs`) | Secondary condition Skip invokes inner without body enforcement. Otherwise capture under the physical cap, materialize only when the inner trait requires it, apply the logical limit and failure policy, then invoke inner on success. Tolerated read failures return Nothing rather than invoking inner; refusal/physical-cap/storage failures retain their hard failure rules. |
| RejectFilter (`reject_filter.rs`) | Log static invalid-plugin names and return a fixed generic 500 body. Shared static Bytes and prepared names require no per-request body construction; publication and last-known-good retention remain separate owners. |
| PolicyBlockFilter (`policy_filter.rs`) | Record the prepared first blocked plugin and existing metric, then return its prepared status and fixed generic body. The block gate precedes invalid-config rejection when both exist. |
| PolicyBypassNote (`policy_filter.rs`) | Iterate prepared bypassed names through canonical logs/metrics and return Nothing. It is an observation entry, not permission to hide another reject gate. |
| RequestHeaderModifierFilter (`gateway_api_filters/request_header_modifier.rs`) | Apply prepared Set, Add and Remove lists in that order. HeaderValue clones supply owned setter arguments; failures retain static diagnostics and subsequent operations continue. |
| RequestRedirectFilter (`gateway_api_filters/request_redirect.rs`) | Borrow live host/path/query and matched-prefix context, build Location with prepared transform policy, log numeric status, construct/write a zero-length response and terminate. Missing context/build/header/write failures return fixed 500. Location must survive the owned response write. |
| URLRewriteFilter (`gateway_api_filters/url_rewrite.rs`) | Compute the optional effective URI, publish it before an optional prepared hostname, and return GoodNext. Missing prefix context or mutation failure returns fixed 500; URI and Host have separate mutation boundaries. |
| GapiCorsFilter (`gateway_api_filters/cors_filter.rs`) | Read live Origin and preflight state. Preflight handles allowed/denied responses locally; ordinary requests defer actual CORS headers to response filtering. Existing write-result and termination behavior remains subject to the common runtime fallback. |
| ExtensionRefFilter (`gateway_api_filters/extension_ref.rs`) | Resolve the request-pinned runtime, enforce target/backend-scope restrictions, establish tokenized nested logs, check live IP dependencies and await inner entries in order. Preserve local outcome-frame ownership, identity/dye installation, anti-spoof stripping, dedicated-log suppression and first non-continuing result. Preparation failure writes the request-stage error response before termination. |

The ordinary PluginRuntime runner returns immediately for an empty input chain.
For a nonempty chain it captures gRPC disposition, prepares the phase's local
rate-limit frame and session adapter, then selects the IP-unavailable gate unless
policy blocking takes precedence. RequestId early/after-early filtering occurs
before each eligible invocation. Entry identity and dye installation/stripping
precede condition/body wrappers. Completed invocations restore identity before
result handling; retained ordinary logs reserve on first insertion. HTTP/gRPC
error conversion, termination framing, stage log completion and frame ending
remain centralized. These normal-return paths are not a claim that every cleanup
callback runs when an awaited future is dropped.

ExtensionRef keeps its runtime Arc across inner awaits and writes completed inner
logs through the existing context-owned token. It does not flatten nested logs
into the outer entry or replace pinned runtime ownership with a short borrow.
The [log-token contract](../../skills/04-review/plugin-runtime/regression-guards/extension-ref-tokenized-plugin-log-contract.md)
and [body materialization contract](../../skills/04-review/http-body/regression-guards/request-body-gate-lazy-materialization.md)
remain authoritative for their respective boundaries.

This reconciliation makes no new runtime change. RequestRedirect's numeric-log
allocation was removed separately in the baseline commit. Wrapper elision remains
[request-01](request-01-conditional-wrapper-dispatch.md); CORS matching/value
transfer, stage-log capacity, live condition dependencies and attached business
execution retain their existing decisions and open inspection scope. No whole
request lifecycle, all-entry coverage or cancellation/transport proof is claimed.

Fresh verification passed 18 conditional-wrapper, 31 body-gate, 65 native-filter
tests and two policy/reject ordering regressions (116 total), formatting,
workspace check/Clippy and agent-doc validation. The wrapper/filter selections
also contain response-stage cases; they are not 116 distinct request-entry proofs.
Existing unrelated warnings remain; no full suite or live integration ran.

## GraphQL, body capture and cache nomination direct-entry inventory

Revalidated at `25eb25bef`: these six entries are disjoint from the ten runtime
entries above, reconciling 16 of the current 60 explicit RequestFilter
implementations. The other 44 are outside this table; later inventories below
reconcile further subsets.
Paths below are relative to `edgion-gateway/src/plugins/http/`.

| Implementation / source | Direct work and ownership boundary |
| --- | --- |
| GraphqlSafelist (`graphql_safelist/mod.rs`) | Run the shared bounded evaluator with the compiled whole-document/selected-operation check, then record allow or return the evaluator's fixed rejection. The parsed owner is consumed inside evaluation; the compiled registration set belongs to the plugin generation. |
| GraphqlGuard (`graphql_guard/mod.rs`) | Inspect the parsed request with the compiled guard, then format scalar shape/cost/measure results into bounded stack scratch and optionally record the cost header. Measure mode does not bypass parser or hard safety limits. |
| GraphqlRateLimit (`graphql_rate_limit/plugin.rs`) | Estimate total/root costs under shared evaluation, record the completed total, copy bounded scalar costs into 16 fixed slots, reject over-capacity reports, then await the canonical Redis limiter. Parsed owners and evaluation permit are released before this Redis await; quota order and failures remain owned by the limiter. |
| GraphqlOperationRouter (`graphql_operation_router/plugin.rs`) | Pin the route Arc and reject an existing jump, evaluate selected metadata against prepared exact/default mappings, validate the named backend, then copy selected name/namespace into the strict owned jump. The returned mapping borrows plugin configuration, not the parsed request. |
| RequestBodyBuffer (`request_body_buffer/plugin.rs`) | Return GoodNext without capturing or materializing bytes itself. The mounted body gate performs ordered shared capture; the plugin explicitly reports that materialization is unnecessary. No second body owner lives in the entry. |
| ResponseCache (`response_cache/mod.rs`) | Pass its cold-compiled policy Arc by reference to nomination and return GoodNext. Production nomination can bypass, reuse an equivalent intent, or invalidate conflicting intent; new state requires an active epoch and a ledger lease before allocation. It is not a cache lookup or an allocation-free no-op. |

All six report requires_materialized_request_body=false. For GraphQL this avoids
an eager body-gate read, not the evaluator's explicit POST materialization. Shared
evaluation independently checks request stage, protocol, method, media and byte
limits, then acquires one of four nonwaiting process-wide permits. POST reads the
canonical request body; GET parses the current query. Parsing and synchronous
computation share the bounded frontend owner and Work counter under panic handling.
Successful evaluation can publish a fixed POST proof when its OnceLock is empty,
then freezes body mutation. GET publishes no POST proof. Later evaluations still
parse and execute their own computation; the earlier proof is not a parsed cache.

Evaluation success is not whole-plugin success: router no-mapping/backend denial
and rate-limit capacity/Redis denial can occur after that freeze. Selected metadata
does not authorize fields or backends by itself. Route Arcs and owned jump fields
retain cross-await/cross-stage ownership; replacing them with short borrows needs
more than observing that the entry's local lookup returns a reference.

ResponseCache's successful proxy tail owns subsequent compatibility checks and
key/origin binding. The state is taken out of context across awaited binding and
restored only on success. Hit lookup, capture admission, integrity, freshness,
actual-origin proof and terminal publication remain separate stages. The
[cache ownership guard](../../skills/04-review/http-body/regression-guards/bounded-response-cache-capture.md)
does not make nomination proof of eventual cache eligibility.

No new runtime edit is selected here. Existing frontend reuse, type-condition and
operation-name storage decisions remain [graphql-01](graphql-01-repeated-frontend-parsing.md),
[graphql-02](graphql-02-prepared-type-conditions.md) and
[graphql-03](graphql-03-operation-name-dedup-storage.md); prepared origin components
remain [cache-06](cache-06-prepared-origin-stamp-components.md). This direct-entry
inventory does not close deeper parser, visitor, Redis, cache or transport work.

Fresh checks passed 44 GraphQL-selected tests, 31 body-gate tests, seven cache
lifecycle tests, 21 cache-storage tests and the mounted RequestBodyBuffer capture
refusal test (104 selected executions), formatting, workspace check/Clippy and
agent-doc validation. These selections include supporting and response cases;
they are not distinct proofs of every request behavior. The broader
[cache admission fixture issue](verification-01-cache-policy-fixture.md) remains
outside this verification. Existing warnings remain; no full suite or live IO ran.

## Routing and request metadata direct-entry inventory

Revalidated at `5b72b1d1e`: these eight entries are disjoint from the preceding
16, reconciling 24 of 60 explicit RequestFilter implementations. The other
36 are outside this table; later inventories below reconcile further subsets. Paths below are relative to
`edgion-gateway/src/plugins/http/`.

| Implementation / source | Direct work and ownership boundary |
| --- | --- |
| DirectEndpoint (`direct_endpoint/plugin.rs`) | Skip matched gRPC routes; extract through canonical key_value, resolve against borrowed route backends and validate endpoint membership/policy. Publish the owned direct preset with its captured proof; only accepted jumps format the address in stack scratch for canonical logs/dye. Missing and invalid input retain separate configured policies. |
| DynamicInternalUpstream (`dynamic_internal_upstream/plugin.rs`) | Read an owned key through session.key_get, borrow optional regex capture, then select a borrowed configured target or direct key. Preserve first-match rule order, no-route/no-backend pass-through and canonical backend authorization. Copy selected target fields into the owned jump; only accepted jumps produce success dye. |
| DynamicExternalUpstream (`dynamic_external_upstream/plugin.rs`) | Extract through canonical key_value, borrow optional regex capture and look up the prepared domain map. Copy domain/SNI into the owned external jump. Apply optional Host override only after acquiring the jump, then format success dye; rejected jumps leave Host unchanged. |
| Canary (`canary/plugin.rs`) | Apply the internal enable switch, borrow the active profile and validate its target with the route's canonical matcher. No route/backend references pass through. Copy target fields for the internal jump; accepted jumps dye the profile name. Missing-profile fail-open and configured invalid-target policy retain their existing branches. |
| RegionRoute (`region_route/plugin.rs`) | Check initialization, retain the effective overlay generation and borrow its table or the base table. Ordered direct/map rules may fall through; hash-range is terminal even on a miss. Borrow extracted input until hashing/logging completes, then perform configured writes, one-hop failover and local pass-through or owned external jump publication. |
| RequestId (`request_id/mod.rs`) | Pass prepared rules and optional default configuration to apply_request_ids, log capacity failure and continue. The production adapter applies/reconciles defaults, then reads each live target/source, applies the rule and reconciles request headers in order. This is separate from core trace propagation and early-stage runner selection. |
| RealIp (`real_ip/plugin.rs`) | Establish the configuration view, borrow the configured header and resolve through the shared policy. Format the typed result before exact textual comparison; call the setter only on change. Production setter owns canonical address replacement and remote-GeoIP invalidation; failed setters log and pass through. |
| GeoIpLocation (`geoip_location/plugin.rs`) | Unavailable references deny even with failOpen. Parse-only initializes both lazy lookup slots without retaining clones. Rule mode obtains only required facts/IPs, borrows lookups inside a decision scope and carries a copyable decision into mutation/logging. Lookup failure precedes unavailable-database handling; CIDR-only sides avoid database lookup. |

Canonical key_value can still own composed values; these entries are not all
borrow-only or allocation-free. Jump presets survive into later upstream stages,
so their required owned fields are not removable merely because selection borrows
configuration. RegionRoute nonlocal selection still parses the selected endpoint
and owns request-derived SNI; local-region success does not acquire a jump.
Canary's internal switch does not undo wrapper-level dye stripping performed
before entry invocation.

No runtime edit is selected in this reconciliation. Existing input/index and dye
decisions remain [routing-01](routing-01-dynamic-internal-input-ownership.md),
[routing-06](routing-06-prepared-dynamic-rule-index.md) and
[dye-02](dye-02-lazy-outcome-value-formatting.md). RegionRoute selection/endpoint
decisions remain region-01/02; the newly isolated static writeback-label tradeoff
is [region-03](region-03-prepared-writeback-log-targets.md). RealIp scratch,
RequestId source evaluation and GeoIP decoding/membership retain their existing
ip-02, trace-02 and geoip-01/02 tasks. Deeper policy, discovery, DNS, trace-ID and
GeoIP dependencies remain open; this table is not transitive coverage.

Fresh verification passed 14 DirectEndpoint, ten DynamicInternalUpstream, seven
DynamicExternalUpstream, 15 Canary, 23 RegionRoute, 16 RealIp and 12 GeoIpLocation
tests, plus 29 supporting request_trace_id tests (126 selected executions).
RequestId has no direct module tests; the supporting selection is not a direct
entry test. Formatting, workspace check/Clippy and agent-doc validation passed.
Existing warnings remain; no full suite or live integration ran.

## Request transformation and validation direct-entry inventory

Revalidated at `cf240a80c`: these five entries are disjoint from the preceding
24, reconciling 29 of 60 explicit RequestFilter implementations. The other
31 are outside this table; later inventories below reconcile further subsets. Paths below are relative to
`edgion-gateway/src/plugins/http/`.

| Implementation / source | Direct work and ownership boundary |
| --- | --- |
| CtxSet (`ctx_set/plugin.rs`) | Process rules sequentially: static value, compiled template or canonical source; then configured extraction, one transform and mapping. Produce an owned String before passing its slice to set_ctx_var, whose production adapter owns the stored key/value. Later rules observe earlier writes. Missing values/defaults and setter failures preserve their existing behavior. |
| ProxyRewrite (`proxy_rewrite/plugin.rs`) | Complete optional bounded JSON patching first, then retain the original path and regex captures for URI, Host, method and add/set/remove header operations in order. URI assembly borrows the current query after substitution and creates an owned URI before mutation. The path snapshot remains necessary for later header substitutions. |
| FormJsonTransform (`form_json_transform/plugin.rs`) | Check media and available length, explicitly materialize and recheck the cap, convert under structural/output limits and transfer the output Vec into Bytes. Keep original Bytes across replacement and content-type update; if the header setter fails, attempt body rollback and reject even if rollback succeeds. Rollback itself may fail. Success returns Nothing. |
| RestToGraphql (`rest_to_graphql/mod.rs`) | Enforce request stage, frozen client method/query, GET body/mutation rules, protocol, encoding and size/media preflight. Acquire a shared nonwaiting evaluation permit, read when needed, parse/bind/serialize and validate the final envelope, then release computation owners before body publication. Publish body, POST method, target URI and JSON content type in that order; success returns GoodNext. |
| JsonSchemaValidation (`json_schema_validation/plugin.rs`) | Claim the one-validator request slot before length/media checks, reject empty/oversized input and explicitly read the shared body. Strictly parse one owned instance and validate against the retained compiled schema using its checked work profile and boolean validator. Fixed mismatch/work/panic errors avoid constructing an attacker-controlled validation error list. Success returns Nothing. |

ProxyRewrite's JSON document remains private through all patch operations,
per-operation shape/work checks and capped serialization. Only successful
computation proceeds to overwrite_request_body; later URI/header mutations are
not part of that atomic computation. Its operation-value and pointer copying
remain the separate [rewrite-02](rewrite-02-json-patch-execution-ownership.md)
decision. Staged URI/header template semantics remain
[rewrite-01](rewrite-01-staged-template-compilation.md). CtxSet extraction/trim and
replacement-result storage remain [ctx-02](ctx-02-extract-trim-projection.md) and
[ctx-01](ctx-01-regex-replacement-storage.md); sequential publication is not
permission to retain session borrows across a setter.

RestToGraphql shares the four process-wide evaluation permits with canonical
GraphQL consumers. Its permit spans async body reading and synchronous
parse/bind/envelope construction; panic handling surrounds the synchronous
computation. Body pointers borrow from the parsed input before bounded binding
produces owned output values; existing KeyGet-owned text transfers into binding.
Final envelope reparsing verifies aggregate structure rather than reusing an
unrelated plugin's parsed owner. The permit and parsed input are released before
the body overwrite await. Later publication setters short-circuit on error and
reject; this is not a transactional rollback of all request fields. The canonical
[adaptation contract](../../skills/04-review/http-body/regression-guards/rest-to-graphql-request-adaptation.md)
retains freeze/refusal behavior for late writes after GraphQL inspection.

The four body consumers explicitly opt out of eager body-gate materialization,
while CtxSet retains the trait default. That opt-out does not eliminate the
consumer's own materialization or authorize a second body cache. Form decoding
already borrows plain components and serializes grouped values directly; parsed
JSON, schema-engine internals, template engines and patch dependencies remain
separate deeper inspection scope. No new runtime change or performance claim is
made in this inventory.

Fresh verification passed 12 CtxSet, 24 ProxyRewrite, 23 FormJsonTransform, ten
RestToGraphql and nine JsonSchemaValidation Gateway tests, plus nine REST binding
and 25 JSON-schema Resources tests (112 selected executions). These include
supporting helper cases, not 112 distinct entry proofs. Formatting, workspace
check/Clippy and agent-doc validation passed with existing unrelated warnings.
No full suite or live integration ran.

## Access policy and local response direct-entry inventory

Revalidated at `0e01c0269`: these six entries are disjoint from the preceding
29, reconciling 35 of 60 explicit RequestFilter implementations. The other
25 are outside this table; later inventories below reconcile further subsets. Paths below are relative to
`edgion-gateway/src/plugins/http/`.

| Implementation / source | Direct work and ownership boundary |
| --- | --- |
| RequestRestriction (`request_restriction/plugin.rs`) | Borrow prepared polarity/status/body and await the canonical compiled condition set. Only True counts as matched before applying Allow/Deny polarity; terminal evaluation results propagate. Invalid policy takes the fixed fail-closed path. Denial clones prepared Bytes into the shared response helper and returns termination, including write failure. |
| RequestAccessPolicy (`request_access_policy/plugin.rs`) | Reject failed initialization, capture a ConfigData view when required and borrow the selected compiled profile. Demand flags gate primary IP parsing and Host normalization; URL candidates produce a request-local match vector. Required groups are ANDed, rules within each group are ordered alternatives, and matching primary rules evaluate supplemental conditions asynchronously. Denial returns prepared body/status through ErrResponse. |
| IpRestriction (`ip_restriction/plugin.rs`) | Ensure the ConfigData view, parse the configured live IP source and evaluate the canonical bound ACL. Reduce the decision before the response await. Missing/invalid IP passes only for Default(Allow); named group decisions retain their diagnostics. Denial transfers a cloned prepared Bytes handle to the shared response helper and terminates. |
| Cors (`cors/plugin.rs`) | Borrow live Origin for prepared configuration matching, then retain an echoed owned origin or literal wildcard across session mutation. Missing Origin passes. Rejected preflight attempts a local 204 and terminates; rejected actual requests pass without CORS headers. Accepted preflight either queues headers and continues or writes a local 204 and terminates; actual requests queue headers and continue. Existing setter/write failure branches remain distinct. |
| Mock (`mock/plugin.rs`) | Await the configured prepared delay, apply configured response headers then Content-Type, ignoring setter errors. Terminating mode returns prepared status and a cloned shared Bytes body with a prepared status diagnostic; nonterminating mode returns GoodNext after header setup. |
| FaultInjection (`fault_injection/plugin.rs`) | Sample independent delay/abort decisions in a scoped RNG block before any await. Ratio endpoints and fixed/equal delay bounds avoid sampling. Await the selected u64-millisecond delay before applying an abort; abort sets Content-Type, appends configured headers and returns prepared status/shared Bytes. No selected fault or delay-only completion continues. |

The restriction response helper builds headers, awaits header/body writes and
shuts down after successful writes. Its callers still return termination on
error; an attempted local write is not proof of a completed response. CORS's
rejected preflight ignores write errors, while accepted preflight propagates
helper errors into termination. Queueing failure in preflight_continue can
therefore terminate before a complete response; canonical runtime handling
remains responsible for that state. This inventory does not redefine transport
success or [preflight continuation ownership](../../skills/04-review/cors/regression-guards/gw-http-proxy-req-01-preflight-result-mapping.md).

Restriction's invalid/unresolved Deny policy is sealed separately from live
condition evaluation and runtime IP-availability gates. Do not turn unavailable
references into a negatable false predicate or collapse all consumers into one
failure rule; the [consumer-specific contract](../../skills/04-review/plugin-metadata/regression-guards/unresolved-condition-references-need-consumer-policy.md)
remains authoritative. RAP borrows its immutable selected profile through
supplemental awaits; its match vector also survives those awaits. These are not
claims that the full condition, reference-publication or matcher dependency
graph has been inspected here.

All six retain the trait's default body-materialization declaration. Prepared
response Bytes avoid per-request configured-body copies but still require owned
handles across response awaits. No new runtime edit is selected: RAP result
storage remains [rap-01](rap-01-url-match-storage.md), address projection remains
[ip-01](ip-01-session-address-projection.md), and CORS derived Max-Age and queued
value transfer remain [cors-01](cors-01-derived-max-age-preparation.md) and
[cors-02](cors-02-session-header-value-transfer.md). Native Gateway API CORS is a
separate implementation already inventoried above. Condition execution and
transport internals remain deeper inspection scope.

Fresh verification passed 18 RequestRestriction, 23 RequestAccessPolicy,
16 IpRestriction, 17 Cors, six Mock and nine FaultInjection tests, plus 70 shared
condition tests (159 selected executions). Supporting tests do not constitute
159 distinct entry proofs. Formatting, workspace check/Clippy and agent-doc
validation passed with existing unrelated warnings. No full suite, live
integration or performance benchmark ran.

## Local authentication and credential staging direct-entry inventory

Revalidated at `147030d72`: these six entries are disjoint from the preceding
35, reconciling 41 of 60 explicit RequestFilter implementations. The other
19 are outside this table; later inventories below reconcile further subsets. Paths below are relative to
`edgion-gateway/src/plugins/http/`.

| Implementation / source | Direct work and ownership boundary |
| --- | --- |
| BasicAuth (`basic_auth/plugin.rs`) | Borrow Authorization for decoding, then move owned username/password and a pinned known-user generation into spawn_blocking. Returned username/group survive that task. Missing credentials may select anonymous access; bad credentials never do. Failure delay precedes attempted authentication response writing; success sets sensitive consumer metadata then optionally removes Authorization. |
| KeyAuth (`key_auth/plugin.rs`) | Select the first nonempty supported Header/Query/Cookie/Ctx source through canonical key_value, hash the key and borrow prepared metadata from the runtime map. Clone prepared sensitive HeaderValues into the setter, then optionally hide only the selected credential source. Missing credentials may allow anonymous access; a supplied invalid key takes delayed rejection. |
| JwtAuth (`jwt_auth/plugin.rs`) | Borrow a token in header, opt-in query, then cookie priority. Synchronous verification routes issuer/group/key and returns owned verified claims with a borrowed group. Success logs the issuer borrowed from immutable claims, projects headers, optionally serializes context claims and then hides the selected source. Missing-token anonymous access remains distinct from invalid-token rejection. |
| JweDecrypt (`jwe_decrypt/plugin.rs`) | Borrow the trimmed selected token, enforce token length and prepared-key state, decode/decrypt into owned plaintext and reject header controls. Publish the sensitive forwarding header before optional JSON claim projection, context storage and source hiding. Strict missing-token rejection differs from nonstrict bypass; failed forwarding stops later consumers. |
| Csrf (`csrf/plugin.rs`) | Read the live method at execution. GET/HEAD/OPTIONS generate and append a new token cookie before continuing. Other methods borrow header/cookie values, require equality, decode an owned token and verify signature/age. Missing, mismatched or invalid tokens return fixed 401 responses without exposing token contents. |
| CredentialInjector (`credential_injector/plugin.rs`) | Check the prepared monotonic expiry, reserved target and all current Connection nominations. Remove the client target value, then stage an owned HeaderName and shared sensitive HeaderValue handle for later attempt-local insertion. Deferral failure rejects; successful staging does not insert the secret into the current request. |

BasicAuth's Arc and decoded credentials are required by the blocking-task handoff;
this inventory does not assert that dropping the request cancels a task already
running. KeyAuth metadata remains tied to the immutable plugin while live setters
consume owned header values. JWT still performs its unverified routing decode and
separate verified decode; borrowing issuer from the success result removes neither.
The [issuer/key binding contract](../../skills/04-review/jwt-auth/mandatory-design/issuer-key-binding.md)
retains algorithm checks, group-scoped keys and verification order. Authentication
response helpers can fail; termination after an attempted write does not establish
successful delivery.

JWE decrypts its ciphertext buffer in place and transfers it into a UTF-8 String;
plaintext still feeds multiple ordered consumers. Its header transfer remains
[jwe-01](jwe-01-plaintext-header-ownership.md), not permission to release plaintext
before optional consumers finish. CSRF checks the current execution point, not a
frozen method or a guarantee against later rewrites; see the
[method-order contract](../../skills/04-review/csrf/regression-guards/execution-time-method-rewrite-boundary.md).
Cookie production remains [csrf-01](csrf-01-cookie-header-ownership.md).

Credential staging preserves the owned sensitive generation across later stages
and retries. The adapter can reject protected transcoder-header mutations. Final
upstream-request processing still rechecks expiry and both Connection views
before inserting credentials under the
[deferred-materialization contract](../../skills/04-review/security/regression-guards/deferred-credential-connection-nomination.md).
Request-stage checks alone are not proof of eventual injection or delivery.

All six retain the default body-materialization declaration. No new runtime edit
is selected here; JWT issuer borrowing is already in the baseline. Existing
[strip storage](auth-01-strip-survivor-storage.md),
[challenge preparation](auth-02-prepared-challenge-generation.md),
[claim paths](auth-03-claim-path-preparation.md) and
[BasicAuth MD5 result storage](basic-auth-01-md5-result-storage.md) decisions remain
open. Crypto/parser engines, credential publication, blocking-pool behavior and
transport dependencies are deeper scope, not closed by this direct inventory.

Fresh verification passed 19 BasicAuth, 15 KeyAuth, 47 JwtAuth, 21 JweDecrypt,
20 CSRF and seven CredentialInjector tests, plus 38 shared-auth helper tests
(167 selected executions). Supporting cases are not distinct direct-entry proofs.
Formatting, workspace check/Clippy and agent-doc validation passed with existing
unrelated warnings. No full suite, live integration or performance benchmark ran.

## Body, certificate and external authentication direct-entry inventory

Revalidated at `1fa2b7ccc`: these four entries are disjoint from the preceding
41, reconciling 45 of 60 explicit RequestFilter implementations. The remaining
15 entries are reduced by the following admission and outbound inventory. Paths below are relative to
`edgion-gateway/src/plugins/http/`.

| Implementation / source | Direct work and ownership boundary |
| --- | --- |
| HmacAuth (`hmac_auth/plugin.rs`) | Run header/credential/timestamp/signed-field/signature checks before optional body capture and digest verification. Retain the verified username and expected digest across body awaits, then reborrow immutable credential metadata for prepared header publication, credential hiding and group-attributed success logging. Capture/logical-cap failures, digest-read failures, absent credentials and invalid authentication retain separate outcomes. |
| HeaderCertAuth (`header_cert_auth/plugin.rs`) | Connection mode clones the existing ClientCertInfo projection; header mode borrows and trims text, decodes the configured representation and checks the leaf against the prepared CA pool before constructing owned metadata. Apply identity or DN/SAN projections and fingerprint headers, then optionally hide the source in header mode. Extraction or identity errors may select configured anonymous access; otherwise reject. |
| ForwardAuth (`forward_auth/plugin.rs`) | Optionally read the current committed body, then borrow current Host/path/method/query metadata into the canonical bounded outbound exchange. Emit collected diagnostics after the await. Success copies configured response fields into request headers and optionally removes Authorization. A rejected body predicate becomes 403; status rejection preserves the auth response status. Denial retains the owned bounded response body/headers across delay and local response writing. |
| LdapAuth (`ldap_auth/plugin.rs`) | Prefer Proxy-Authorization over Authorization and reject a malformed selected header without falling through. Decode one owned credential buffer and retain username/password ranges, build the escaped owned bind DN and borrow both across connection/bind awaits. Success publishes sensitive consumer metadata and hides configured credentials. Missing credentials may be anonymous; invalid credentials and service/TLS/timeout failures retain separate rejection paths. |

HmacAuth is an explicit exception in PluginRuntime::use_generic_body_gate. Its
trait default must not be read as proof of generic eager capture. The dedicated
verifier owns the resolved requirement, enforces capture and logical limits and
hashes the current immutable body without forcing disk-backed storage into a
contiguous buffer. The [cheap-auth-before-body contract](../../skills/04-review/hmac-auth/regression-guards/request-body-hash-after-cheap-auth.md)
remains authoritative. Missing authentication can be anonymous. After an auth
header is found, no usable configured credentials returns the fixed internal
failure; supplied invalid credentials never select anonymous access.

HeaderCertAuth's configured anonymous policy also covers extraction/identity
failures; it is not the missing-only policy used by several other authenticators.
Its sensitive header setter errors are currently ignored, distinct from explicit
identity-validation errors. Header-mode checks are the existing configured-CA
leaf checks, not proof of a new full-chain verifier or trusted header origin.
Connection projection and CA public-key preparation remain
[tls-04](tls-04-header-cert-connection-projection.md) and
[tls-03](tls-03-header-ca-public-key-preparation.md).

ForwardAuth's current metadata differs from omitted auth-hop defaults, which
use entry identity. Denial copying preserves repeated configured fields subject
to protected-header and Connection-nomination filtering; local write attempts
are not delivery proof. Its query borrow is already in the baseline, while
[denial-header preparation](forward-auth-01-prepared-denial-header-names.md)
remains deferred. LDAP uses the canonical dial-time EgressResolver and admitted
socket addresses while retaining the original hostname for TLS. Connection and
bind have separate timeout scopes; unbind is another awaited operation, not a
proof of one end-to-end deadline or cancellation cleanup. Preserve the
[dial-time egress contract](../../skills/04-review/ldap-auth/regression-guards/dial-time-egress-policy.md).

No new runtime change is selected here. LDAP DN preparation remains
[ldap-01](ldap-01-bind-dn-template-preparation.md); HMAC membership, MAC state,
signing-input and missing-field diagnostics retain hmac-01 through hmac-04.
Crypto/certificate parsers, LDAP driver lifecycle, outbound transport and shared
body-storage internals remain deeper scope; this table does not close them.

Fresh verification passed 36 HmacAuth, 20 HeaderCertAuth, 13 ForwardAuth and
28 LdapAuth tests, plus 31 body-gate and 46 network-free outbound-call tests
(174 selected executions). Supporting cases are not distinct entry proofs.
Formatting, workspace check/Clippy and agent-doc validation passed with existing
unrelated warnings. No full suite, live LDAP/HTTP integration or benchmark ran.

## Admission and outbound request direct-entry inventory

Revalidated at `d7b6331a4`: these five entries are disjoint from the preceding
45, reconciling 50 of 60 explicit RequestFilter implementations. The remaining
10 entries are reduced by the following OIDC and ExtProc inventory. Paths below are relative to
`edgion-gateway/src/plugins/http/`.

| Implementation / source | Direct work and ownership boundary |
| --- | --- |
| RateLimitLocal (`rate_limit_local/plugin.rs`) | Resolve an owned key through the shared helper, read the prepared interval and current effective rate, then observe the per-plugin lazy CMS. Publish a configured outcome slot before Continue, rejection or normal headers. Key-miss Allow bypasses observation; key-miss Deny uses zero retry delay. Logs expose key length, not identity. |
| RateLimitRedis (`rate_limit_redis/plugin.rs`) | Prepare ordered quota jobs with owned bounded keys before client lookup. Pin one healthy client and check jobs sequentially under the optional data-only timeout. Stop at first denial/error; select one checked result for headers. Release jobs before response awaits, both after checked decisions and before unavailable-client failure handling. The latter release was added after this inventory baseline. |
| WebhookKeyGet (`webhook_key_get/plugin.rs`) | Resolve configured variables sequentially through the canonical bounded outbound call. Emit diagnostics after each await, borrow the owned extracted value or configured default into the context setter, then proceed to the next variable. A resolved empty string wins over default; absent resolution without default skips publication. Setter errors are logged and processing continues. |
| RequestMirrorPlugin (`request_mirror/plugin.rs`) | Apply target/sampling checks and nonwaiting instance/process task admission. Build an owned URL from borrowed current path/query before snapshot awaits or task handoff. The detached writer owns prepared headers, trace identity, permits and either an isolated committed snapshot or the bounded tee receiver. Runtime skips continue the primary request. |
| AllEndpointStatus (`all_endpoint_status/plugin.rs`) | Claim the configured process interval before acquiring the global semaphore with a five-second wait limit. Capture discovery/policy generations, collect endpoint projections and borrow current path/query during fan-out. Selected probes retain owned results for ordered assembly and JSON response writing; the global permit spans local response handling. |

The local limiter's interval is prepared, but Cluster-scope effective rate still
reads the current instance count. Its lazy estimator remains
[rate-limit-01](rate-limit-01-local-estimator-initialization.md). Continue publishes
the local outcome without normal limit headers or rejection; it does not refund
CMS observations or guarantee that a later Redis consumer runs. Preserve the
[local-overflow contract](../../skills/04-review/rate-limit/regression-guards/local-allowance-redis-overflow.md).

Redis's timeout excludes key preparation and local response writing. Sequential
quota checks can retain earlier debits after later denial, failure or timeout;
this is not a transaction or refund contract. At the inventory baseline, the early unavailable-client
branch retained jobs across its failure-policy await. A later narrow correction
explicitly releases them first, aligning both branches with the
[named-quota ownership page](../../skills/04-review/rate-limit/regression-guards/redis-named-quotas.md). Existing redis-01 through redis-07
investigations remain deferred. Attempted rejection writes do not prove delivery.

Webhook results remain owned across resolver return, then are copied into request
context by its setter. Later variables can observe earlier publications. Its
existing fail-soft contract does not authorize parallel resolution. JSON projection
and extraction-path preparation remain [webhook-01](webhook-01-selective-json-projection.md)
and [webhook-02](webhook-02-extraction-path-preparation.md).

Mirror snapshots and headers-ended requests disarm the body cancellation guard
and detach; other requests install streaming state. Rejected installation drops
the armed state and cancels the writer. The production exchange drops the response
after headers and never aggregates its body. The
[mirror budgets](../../skills/04-review/request-mirror/regression-guards/mirror-resource-budgets.md)
bound retained mirror payload, not total RSS. Current query borrowing is already
in this baseline; the network-free writer seam does not verify wire behavior.

Endpoint fan-out has its own wall timeout and per-probe timeout; neither covers
all preceding admission/collection work or the final local response write.
Completed partial results survive the fan-out deadline. The selected endpoint cap
does not bound all preselection projections: [endpoint-02](endpoint-02-bounded-endpoint-projection.md)
remains open, alongside endpoint-01, endpoint-03 and endpoint-04. Preserve the
[per-invocation TLS identity contract](../../skills/04-review/backends/regression-guards/all-endpoint-status-tls-identity.md).

Fresh verification passed four local-limiter, 24 Redis-limiter, four WebhookKeyGet,
28 RequestMirror and 24 AllEndpointStatus tests, plus 11 webhook-resolver tests
(95 selected executions). Supporting cases are not distinct entry proofs.
Formatting, workspace check/Clippy and agent-doc validation passed with existing
unrelated warnings. No full suite, live Redis/HTTP integration or benchmark ran.
Deeper algorithms, client lifecycle, transport and body storage remain open.

## OIDC and ExtProc request direct-entry inventory

Revalidated at `00776bd1e`: these two entries are disjoint from the preceding
50, reconciling 52 of 60 explicit RequestFilter implementations. Eight entries
were outstanding before the following engine inventory: DslPlugin, WafPlugin, WasmPlugin, Arc<WasmPlugin>, GrpcJsonTranscoder,
AiProxy, AiGuard and Guardrail. Paths below are relative to
`edgion-gateway/src/plugins/http/`.

| Implementation / source | Direct work and ownership boundary |
| --- | --- |
| OpenidConnect (`openid_connect/request_filter_impl.rs`) | Handle logout before callback, then select bearer or session authentication. Callback code/state, bearer tokens and decoded session/raw-cookie values remain owned across mutable-session operations and awaits. Session resolution, refresh, verification and optional userinfo precede header projection and Gateway-cookie stripping on successful continuation. |
| ExtProc (`ext_proc/plugin.rs`) | Create and install one attached Arc owner with client/mode/identity state before dispatch. Streamed mode checks the transport header-END_STREAM fact and either performs a header-only exchange or installs the request-body handler after its header verdict. Buffered mode captures the canonical immutable body before headers, but constructs the protobuf Vec only after the body phase is admitted. |

OIDC logout takes the ID token for redirect construction, awaits best-effort
revocation, removes cached access-token state and attempts a redirect with cleared
cookies. Callback handling requires nonempty code and state. Missing bearer-only
credentials take delayed rejection; session mode instead distinguishes cache-miss,
preemptive and expiry refresh. The raw cookie supplies a lazily computed singleflight
key. A failed preemptive refresh can still use a valid token. Expired-token grace
is restricted to the expiry-refresh wait-timeout branch and requires verification
with the separate expiry-leeway check; it is not general dependency fail-open.

Session recovery can reach configured Pass/Deny/Auth fallback. Userinfo and
revocation remain best-effort. The ordinary bearer path preserves its own
401/403 versus dependency-502 routing and delay boundary. Header publication is
bounded and best-effort, not proof that every requested projection was inserted.
Gateway cookies are stripped on every continuation path under the
[cookie ownership contract](../../skills/04-review/oidc/regression-guards/session-cookie-upstream-ownership.md).
When a fresh Authorization value is configured, bearer hiding does not remove it.
Attempted redirects and authentication replies do not prove successful delivery.

The generation-owned [signing policy](../../skills/04-review/oidc/regression-guards/prepared-signing-policy.md)
is already prepared before dispatch. Claims serialization, cookie formatting,
scope membership, introspection ownership, prepared JWKS keys and borrowed
cookie projection remain oidc-01 through oidc-07. No new token-borrowing interface
or runtime change is selected; cache, refresh-lock, crypto and HTTP internals
remain deeper scope. Preserve the caller-specific
[dependency error projection](../../skills/04-review/oidc/regression-guards/dependency-error-projection.md).

ExtProc's attachment identity and Arc survive into response companions and body
handlers. Missing transport capability, duplicate attachment and failed handler
installation retain configured failure handling. Header END_STREAM is a transport
fact, not a Content-Length inference. The owner validates matching verdicts and
allowed mode overrides; request-header Continue-and-Replace stops the buffered
body direction, while streamed replacement is rejected. Immediate-response handling
and terminalization remain separate from successful local response delivery.

The buffered capture retains the original body generation across header verdicts.
Skipping/replacing that direction avoids its protobuf conversion. Body publication
precedes header mutation within the applicable verdict, but previously accepted
phase mutations are not rolled back after later failure. Preserve the
[interactive phase contract](../../skills/04-review/ext-proc/regression-guards/ext-proc-interactive-continue-and-replace.md).
Terminal stream retirement, codec policy, protobuf body storage and local-reply
ownership remain extproc-01, extproc-02, body-02 and response-03 respectively.

Fresh verification passed 103 OIDC plugin tests and nine ExtProc buffered-request
tests (112 selected executions). The streamed direct path was source-inspected;
these selected ExtProc tests do not establish fresh streamed or wire acceptance.
Formatting, workspace check/Clippy and agent-doc validation passed with existing
unrelated warnings. No full suite, live IdP/gRPC integration or benchmark ran.

## Embedded engine request direct-entry inventory

Revalidated at `696adcb11`: these four entries are disjoint from the preceding
52, reconciling 56 of 60 explicit RequestFilter implementations. The remaining
four are reconciled in the following AI and transcoder inventory. Paths below are
relative to `edgion-gateway/src/plugins/http/`.

| Implementation / source | Direct work and ownership boundary |
| --- | --- |
| DslPlugin (`dsl/plugin.rs`) | Borrow the configured VM, error policy and runtime into execute_safe. The VM creates invocation-local execution state while reusing prepared script/constants. The entry opts out of eager body materialization; individual body operations retain their own capture/read boundaries. |
| WafPlugin (`waf/mod.rs`) | Claim the request phase for the selected owner, construct a fresh transaction from borrowed header descriptors and owned temporary URI/request-line values, then drop those temporaries after transaction construction. Execute request-header rules before body inspection. Successful request phases install the owned transaction for later response processing. |
| WasmPlugin (`wasm/plugin.rs`) | Await the loaded module generation, acquire an exclusive lease and install a request binding. Loan that execution while building callback snapshots, invoking the guest, awaiting permitted callouts and optionally processing the complete request body. Successful callback completion returns execution to its binding for later hooks. |
| Arc<WasmPlugin> (`wasm/attached.rs`) | Delegate by shared reference to WasmPlugin::run_request. This is a distinct explicit trait implementation, not a second VM, independent body capture or additional execution binding. |

DSL compilation, static validation, runtime limits, Redis alias mapping and rate
configuration belong to construction. Borrowing those owners does not make VM
execution allocation-free: invocation state and builtin results have their own
lifetimes. execute_safe catches unwinding from the execution future and applies
the configured error policy to ordinary runtime errors/panics. BodyAccess errors
instead record fail-close and return the fixed 500 response. DSL representation,
JSON and outbound-client decisions remain dsl-01 through dsl-10; this inventory
does not close builtin or leaf compiler dependencies.

WAF keeps the captured immutable compiled generation through its transaction.
Phase claims can skip or fail before construction. Header views preserve the
existing HTTP/2 authority-to-Host synthesis; transaction-owned bytes must outlive
the live session views. Descriptor transport remains [waf-12](waf-12-head-view-transport.md).
Request-header intervention returns before body work. Format-specific limits,
full structured JSON/XML inspection, raw capture and explicit prefix inspection
retain distinct paths and failure outcomes. Physical limits, inspection windows,
storage limits and completion evidence are not interchangeable. Failed transaction
installation returns 500; no cross-generation transaction reuse is introduced.
Existing WAF matcher, capture and result-storage decisions remain deferred.

Wasm header snapshots and properties are owned by callback frames. Paused
callouts retain the exclusive lease and use existing request deadlines;
fire-and-forget callouts are rejected. A clean header result precedes the optional
whole-body hook. Captured Bytes are copied into the guest-frame Vec, and one
terminal request-body callback runs even for an empty body. A staged replacement
is published through canonical request storage before pending header operations;
a deliberate local reply skips irrelevant body publication. NotBuffered and
storage failures return 500, BufferLimitExceeded returns 413, and other read
failures retain the configured failure policy with bounded diagnostics.

Only DSL overrides requires_materialized_request_body to false; WAF and both
Wasm implementations inherit the trait default. Optional wrapper capture and
plugin-specific lazy capture must therefore be distinguished. The
[Wasm lifecycle contract](../../skills/04-review/wasm/regression-guards/request-execution-lifecycle.md)
owns cancellation, terminal cleanup, generation pinning and admission. Releasing
host callback payloads does not scrub guest memory; instance/guest budgets are
not total RSS bounds. Existing wasm-01 through wasm-07 decisions remain open.

Fresh verification passed ten DSL plugin, 52 Gateway WAF, five attached-Wasm and
eight Wasm request-body tests (75 selected executions). This is supporting unit
evidence, not complete engine, callout, transport or performance coverage.
Formatting, workspace check/Clippy and agent-doc validation passed with existing
unrelated warnings. No full suite, live network integration or benchmark ran.
No runtime change or deferred-task closure is selected by this inventory.

## AI and transcoder request direct-entry inventory

Revalidated at `543e4fcc8`: these four entries complete reconciliation of all
60 explicit RequestFilter implementations in 59 source files. This closes the
direct-entry inventory, not the goal or their reachable dependencies. Inherited
trait defaults, dispatch wrappers, later callbacks and deferred optimization
choices retain their separately recorded scopes.

| Implementation / source | Direct work and ownership boundary |
| --- | --- |
| GrpcJsonTranscoder (`plugins/http/grpc_json_transcoder/plugin.rs`) | Check conversion scope and synchronously bind current method/path/borrowed query into owned fields. Own capture admission to preserve REST errors, check input/media limits, acquire conversion reservation and encode bounded input. Publish the framed body, upstream method/URI and headers before installing the response conversion plan. |
| AiProxy (`ai/http/proxy.rs`) | Capture client API, route and one backend snapshot, then validate endpoint/body generation and canonicalize the request. Build model/policy/credential candidates and a scheduler, await configured quota admission, recheck body generation and install the owned attempt plan. The entry delegates through the global store and per-invocation entropy helper. |
| AiGuard (`ai/guards/request/plugin.rs`) | Mark actual invocation before applicability checks, skip a settled entry identity and reject execution after an AI attempt plan. Read the canonical body, select bounded spans and evaluate prepared rules. An optional evaluator receives locally masked text unless local rules already reject. Publish replacements against the captured generation, then record the settled decision/generation. |
| Guardrail (`ai/guards/semantic/plugin.rs`) | Reject unsupported native guard plans, install one attached Arc owner with prepared head limits and tag the plugin log with its entry identity. Duplicate claims record failure on the existing owner and return fixed 500 regardless of fail-open settings. Semantic response decisions occur in the response companion, not this entry. |

Paths are relative to `edgion-gateway/src/`. The transcoder query borrow is already
in this baseline. Bound fields and the compiled binding outlive the query source;
conversion admission remains after capture/size/media checks. Encoding is caught
for unwind isolation. Body/method/URI/header publication is ordered but not a
whole-request rollback transaction. The installed plan retains compiled state,
binding and reservation under the
[transcoder contract](../../skills/04-review/h2-grpc/regression-guards/grpc-json-transcoder-admission-and-outcomes.md).
Successful installation returns Nothing; it is not proof of upstream delivery or
valid terminal gRPC status. Codec, metadata and response framing remain deeper scope.

AiProxy's model-list branch is distinct from configured POST endpoints. Backend
publication skew, invalid declared shares, native guard incompatibility,
continuation, model eligibility and retry bounds are checked before publishing a
ready plan. Canonicalization may rewrite a body privately; the plan records whether
its bytes still match shared storage. Quota downgrade re-resolves candidates and
readmits under the changed logical model. Captured policies, scheduler and quota
ownership survive later attempts; the final post-await body-generation check must
not label old bytes with a new generation. Saved canonical path/query Strings are
plan-owned cross-stage data, not temporary lookup copies. Actual credential
admission, endpoint resolution and transport commit remain later boundaries; see
[prepared attempt ownership](../../skills/04-review/ai-routing/regression-guards/prepared-attempt-owns-admission-input.md).

AiGuard's applicability is POST plus the /chat/completions suffix, whereas AiProxy
uses its configured exact endpoint. Invocation marking precedes that gate so a
later rewrite cannot conceal native guard incompatibility. Owned body/spans stay
alive across evaluator awaits; borrowed rule spans do not replace them. Local
rejection avoids the external evaluation. Replacement validation preserves whole
quoted-token ranges, overlap checks, generation-checked publication and physical
body limits. Pass/Reject do not independently recheck the captured generation;
settlement records the current generation for AiProxy's later comparison.
Refusal parsing remains [ai-guard-01](ai-guard-01-refusal-facts-parser.md).

Guardrail's owner and identity enable later response-header/body/terminal work.
Installing them neither performs an evaluator call nor proves a response is
applicable. The companion checks status, SSE type, identity encoding and the
canonical AI processor before installation. Held-header storage, decision encoding
and callback output capacity remain guardrail-01 through guardrail-03. No new
whole-response buffer, cross-stage cache or guard failure policy is introduced.

Fresh verification passed 31 Gateway transcoder, 21 AiProxy, 31 AiGuard plugin
and two Guardrail plugin tests (85 selected executions). These supporting tests
do not prove complete parser, quota, engine, network or later-phase coverage.
Formatting, workspace check/Clippy and agent-doc validation passed with existing
unrelated warnings. No full suite, live integration or benchmark ran. No runtime
change or deferred-task closure is selected by this inventory.

## Service endpoint selector dependency boundaries

Revalidated at `815ab6312`. This names the inspected selection path without
claiming complete discovery, transport or dependency implementation coverage.

| Boundary | Current source evidence | Remaining decision or limit |
| --- | --- | --- |
| BackendRef selection | Route-owned BackendSelector uses a direct single slot or WeightedRoundRobin over original weighted slots. | Separate from Service endpoint strategy; authorization and resolution follow existing route rules. |
| Native Service peer capture | upstream_peer reaches backends::try_get_peer and CapturedServiceSelection. One membership-matched runtime snapshot supplies the section and selection; HTTP/gRPC policy is captured separately. | Policy publication, transport interleavings and global shutdown are not proved by this inspection. |
| RR endpoint selection | RrCore uses RoundRobinSelector's cold expanded index array, shared atomic cursor and bounded unique fallback. Candidates and final winner use the canonical address resolver. | [backend-01](backend-01-selected-slot-lookup.md) retains winner/slot projection choices. |
| Nonempty CH key | ChCore uses the prepared ring; pinned Continuum hashes the key once, binary-searches and returns borrowed addresses while advancing a wrapping cursor. Edgion resolves original indices before visited checks. | [lb-05](lb-05-consistent-hash-visited-storage.md) covers visited storage; [lb-06](lb-06-ketama-candidate-index.md) covers earlier vnode projection. Empty keys use the already-built RR fallback. |
| LeastConn/EWMA | P2C samples distinct candidates, reads live metrics, checks preferred then alternate eligibility, and falls back to a linear health-only scan. The direct estimator read/update was separately inspected. | Bounded RNG mapping is inspected in [lb-07](lb-07-prepared-p2c-range-sampling.md); the [thread-owner/refill boundary](#p2c-thread-rng-owner-and-refill-boundary) is inspected below. Primitive/OS entropy internals, failure-policy impact and full contention/performance validation remain open. [lb-01](lb-01-precomputed-ewma-decay.md) remains deferred. |
| Eligibility and degrade | CapturedServiceSelection counts survivors with the quiet predicate, then either bypasses eligibility in degrade mode or performs live per-candidate checks with rejection recording. | [backend-02](backend-02-runtime-eligibility-lookup.md) remains deferred; no eligibility result is cached by this audit. |
| Cold section construction | materialize_sections reuses aliases and materialize_section compiles all four selectors from ready backends. Numeric/Bucket staging release is recorded in INDEX. | Complete normalization/publication lifecycle and external Backend extension behavior remain separate. |
| L4 Service consumers | TCP, UDP and TLS passthrough call select_service_endpoint with no endpoint-policy override and live degrade policy. Retained TLS Terminate code has the same call shape but product admission disables it. | This is not authorization to enable Terminate or extend endpoint policy to L4. |

The architecture page now distinguishes Service RR from BackendRef weighting,
corrects P2C ordering/fallback and removes the obsolete unsectioned projection
and L4 helper names. No selector, health observation, policy or API changed.
Fresh tests passed 41 selection, eight compiled-selector, 27 BackendRef-selector,
12 Service-selector and 21 EWMA tests (109 distinct tests), plus formatting,
workspace check/Clippy with existing warnings and agent-doc validation. Tests
support the named behavior boundaries, not a completion audit of the entire goal.
No full suite, live transport, benchmark or dependency implementation test ran.

## P2C thread RNG owner and refill boundary

Inspected at `294ae9856` with rand 0.9.5, rand_core 0.9.5, rand_chacha 0.9.0
and getrandom 0.3.4. This is a direct ownership/lifecycle inspection, not a
cryptographic or operating-system implementation audit.

select_p2c returns for empty/singleton input before rand::rng. For larger lists,
it acquires one ThreadRng handle outside the retry loop. Even max_retries == 0
acquires that handle before the final health-only scan, although it draws no
random words. The compiled production caller supplies 256 retries.

| Operation | Pinned source evidence | Optimization implication |
| --- | --- | --- |
| First thread-local access | ThreadRng initializes ReseedingRng from OsRng and creates Rc<UnsafeCell<...>>. Initialization can fail with a dependency panic. | Do not claim every selection or the first selection is allocation/entropy-free. No warmup or failure-policy change is selected. |
| Later rng() calls | Clone the existing Rc handle; do not clone the generator or its block buffer. ThreadRng is not Send/Sync. | No new generator owner or heap allocation arises from the warmed handle clone itself. Moving ThreadRng into shared MetricCore is not an ownership-preserving cache substitution. |
| Random-word consumption | ReseedingRng delegates to BlockRng, which consumes inline Array64<u32> results and refills on exhaustion. ChaCha12Core supplies the 256-byte result block. | Buffering already spans selection calls and other consumers on the same thread; this is not a per-request Vec or RNG rebuild. Primitive refill internals are a separate boundary. |
| Reseeding | The 64 KiB generated-byte quota is decremented by block generation. At the next exhausted-quota refill, the core reseeds through OsRng before generating another block. | Quota is shared thread consumption, not a request counter or a promise of entropy IO on every selection. Reseed errors panic in this pinned dependency; that observation is not acceptance of a Gateway failure policy. |
| Entropy bridge | OsRng delegates its fallible fill to getrandom::fill. | OS/backend behavior, failure injection and full callback panic impact are not verified here. |

This inspection does not justify a new global RNG lock, shared ThreadRng owner,
custom generator, reseed suppression, or precomputed random choices. Each would
change ownership or sampling semantics without demonstrated benefit. The separate
[bounded-range preparation proposal](lb-07-prepared-p2c-range-sampling.md) remains
unselected. No per-request instrumentation or new RNG abstraction was added.

Fresh unchanged-runtime checks passed six P2C and eight compiled-selector tests,
formatting, workspace check/Clippy with existing warnings and agent-doc validation.
Those exercise selection behavior, not deterministic reseed counts, first-use
allocation, entropy failure or cryptographic correctness. No lifecycle fault
injection, allocation/latency benchmark, full suite or live transport test ran.
Source references are rand src/rngs/thread.rs and reseeding.rs, rand_core
src/block.rs and os.rs, and rand_chacha src/chacha.rs. Re-evaluate these conclusions
when dependency versions/features or the P2C handle lifetime change.

## RequestId admitted source adapter boundaries

Revalidated at ced96f648. This inventory follows configuration validation before
interpreting the generic adapter dispatch. RequestIdConfig::validation_error,
RequestId::create and CompiledRequestIdRule::new restrict admitted sources to the
following set; the latter two independently validate runtime construction.

| Source | Direct adapter behavior and ownership | Early attachment |
| --- | --- | --- |
| None | No source read; target validation and configured generation remain canonical apply_rule work. | Allowed |
| Header | single_header requires exactly one nonsensitive value and clones its HeaderValue. It does not substitute generic first-header KeyGet lookup. | Allowed |
| TraceId | Read the canonical request-owned W3C ID, copy to String and move into HeaderValue. Suppressing x-trace-id does not remove this identity. | Allowed |
| Ctx | Read the current context map, copy its value and move into HeaderValue. Later rules observe current state rather than an entry snapshot. | Rejected |

The other nine KeyGet variants (RemoteIp, DirectPeerIp, Host, Cookie, Query, Path,
RequestPath, Method and RemoteIpAndPath) are rejected as RequestId sources.
They remain valid generic adapter cases for other consumers, not extra RequestId
coverage. Public fields can support synthetic mutation; this reachability claim
is scoped to canonical admitted production configurations.

The early validator permits only Header/Method/TraceId key conditions and rejects
other conditions plus body/dye wrappers. plugin_store passes its result into
configure_request_id_scope; run_early_request_ids checks the invalid flag before
dispatch. This is a Gateway gate as well as a schema/Controller rule.

apply_request_ids eagerly reads target/source before apply_rule, including cases
where the core subsequently preserves an ID or rejects a new slot. The default
patch and every rule outcome reconcile managed headers, so later Header sources
see earlier writes/removals. Moving out identity or snapshotting all sources
would change this boundary. The [deferred source-evaluation task](trace-02-request-id-source-evaluation.md)
now records the actual admitted set and withdraws its prior RequestId Query-cache
premise. The generic pre-capture query-cache caveat is separate.

This closes the direct source-adapter inventory, not all generic KeyGet consumers,
query decoders, protocol header storage, RNG internals or policy publication.
No runtime change or selected lazy-source implementation follows from this audit.
