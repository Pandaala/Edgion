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

Source sets rechecked at `477b5fe30`: the override and inherited tables still
match the source exactly (30 and 22, no missing or stale methods). RequestFilter
source enumeration still finds 60 explicit implementations in 59 files; this
count refresh does not extend their previously scoped implementation coverage.

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
is [region-03](../../skills/04-review/routing/regression-guards/region-route-effective-table-validation.md). RealIp scratch,
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
Cookie production remains [csrf-01](../../skills/04-review/csrf/regression-guards/set-cookie-append.md#csrf-cookie-producer-ownership-csrf-01).

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

## H2 asynchronous body payload ownership

Source revalidated at Edgion `0d0a7c2d3`, locked Pingora
`1aaa45cc70430bc782990bc20ddee3fef36d6b99`, h2 0.4.19 and bytes 1.12.1.
This covers the direct asynchronous receive adapters and writer handoff, not
all dependency codecs, queues, poll-based APIs or transport interleavings.

The h2 RecvStream::data API returns owned Bytes. In the fork's
pingora-core/src/protocols/http/v2/client.rs, read_response_body returns that
owner after releasing received capacity and updating its payload counter.
Capacity-release errors propagate there. Its EOF/trailer validation is retained;
an empty DATA value is not automatically interchangeable with clean EOF.

In v2/server.rs::read_body_bytes_inner, the received owner remains intact through
payload accounting and optional retry-buffer writes. The server ignores the
capacity-release result and releases credit before the cancellable capture await,
so cancellation cannot strand that consumed chunk's credit. Capture/finish poison
and terminal observations remain separate from ownership. Unlike the H1 adapters,
these receive handoffs do not copy a reusable reader slice into a new Bytes.
Retry capture or other consumers can still have their own storage costs.

The proxy H2 response pump moves received data into HttpTask::Body and its channel
after existing empty/EOS checks. The request path's send_body_to2 first runs the
canonical relay callback path, then passes owned data through
write_upstream_body_watching_stall into the core v2 writer. The watcher retains
its pinned write across probe ticks; restarting it would lose partial-write state.
This source trace does not establish complete timeout/cancellation correctness.

The shared v2::write_body owns remaining Bytes. For nonempty output it reserves
capacity, awaits a grant, splits min(remaining length, grant), then transfers the
piece to SendStream::send_data. END_STREAM accompanies only the final piece when
requested. Empty output is sent directly without waiting for zero capacity.
bytes 1.12.1 split_to already moves the whole owner when splitting its full length;
partial splits clone the handle and adjust ranges without copying payload bytes.
Handle metadata/refcount work can remain, so this is not an allocation-free claim.

No additional safe copy-removal candidate was selected at these boundaries.
The [H1 proposal](transport-01-h1-body-buffer-handoff.md) concerns different reader
storage. Delaying H2 credit until Bytes destruction or bypassing callback/writer
owners would change lifecycle and backpressure policy, rather than remove an
established redundant copy. No such redesign was proposed in this audit.

Fresh Gateway selections passed 19 request-body-action, 31 upstream response-body
and 11 upstream trailer-filter tests. Formatting, workspace all-target check/
Clippy and agent-doc validation passed with existing warnings. These are current
Gateway regressions, not fresh fork-suite, live flow-control, allocation or
throughput results. No runtime, dependency or test fixture changed.

## DNS query ownership and dependency boundaries

Direct source revalidated at `558729857`. This follows native upstream selection,
AI external endpoint selection and Common egress adapters into the shared DNS
core; it does not close every DNS consumer or dependency lifecycle.

| Boundary and source | Ownership and retained work |
| --- | --- |
| Gateway `routes/http/proxy_http/pg_upstream_peer.rs` to `backends/mod.rs` | Native ExternalName self-resolution uses the captured Service projection and resolver inputs. The EndpointSlice strategy is separate. `resolve_external_ips_with` bypasses resolver/cache for bare IP literals; otherwise it uses the configured cache switch and captured TTL. Native peer construction selects the first safe address, retaining the complete result for rejection diagnostics. |
| Gateway `backends/mod.rs` resolver selection | Service, Gateway and EGC candidates are borrowed in precedence order. Routed external AI omits the Service layer. `httpdns_config_for_ref` still formats a lookup key and reads the live typed LinkSys table; candidates do not pin a provider generation before that lookup. |
| Common `dns/resolver_select.rs` and `dns/mod.rs` | The first configured candidate wins. Only selected inline servers are cloned into the owned spec; an empty selected list or missing/wrong-provider HTTPDNS reference yields System rather than trying a lower layer. Existing failed HTTPDNS preparation retains its declared fallback. System/prepared HTTPDNS fingerprints borrow identity; Standard DNS constructs one owned output buffer. |
| Gateway `backends/discovery/dns_cache.rs` and `backends/mod.rs::resolve_external_ips_with` | Lookup borrows service/hostname/fingerprint key components, while the LRU returns an owned cloned entry/address vector. A successful miss clones addresses into the cache and returns the original vector. Capacity, strict entry expiry and query identity remain separate from address representation; see [DNS result ownership](dns-01-address-result-ownership.md). |
| Gateway `ai/http/attempt.rs::resolve_ai_endpoint` | Common DNS returns all addresses, the Gateway egress filter evaluates the vector, then the first surviving address becomes the endpoint. This does not call the native Service DNS cache. Early first-address projection remains [deferred](dns-02-ai-first-address-projection.md), including rejection-observation semantics. |
| Common `dns/system.rs`, `standard.rs`, `hickory.rs` | System fast-paths bare literals, otherwise constructs host:0 and preserves first-seen IP order while deduplicating. Standard DNS parses servers and builds a resolver per query; accepted server order and duplicates survive parsing. [System input ownership](dns-05-system-query-input.md) and [resolver preparation](dns-03-standard-resolver-preparation.md) require separate decisions. An empty direct Standard list uses System; an empty explicit HTTPDNS DNS fallback rejects. |
| Common `dns/httpdns.rs` and `http_fetcher.rs` | Queries borrow prepared response metadata and reuse the prepared client. URL rendering remains per query; the rendered owner is dropped after successful fetching, before parsing. The fetcher classifies normalized literals, sends that same parsed URL, strips URLs from SDK errors and enforces the byte cap while receiving. The returned String owns the receive buffer. Parsing preserves ordered deduplicated results under the 256-IP cap; full JSON projection remains [deferred](dns-04-json-response-projection.md). These are byte/result bounds, not a total heap or concurrent-query bound. |
| Common `egress.rs` and Gateway `plugins/http/common/outbound/egress.rs` | Each shared Inner owns its policy/spec and a 16-entry memo. Zero TTL bypasses it; fresh hits clone addresses. Fresh DNS results are filtered/observed before memo storage, including empty negative results; DNS errors are not cached. Full-map replacement reuses a borrowed-key lookup; pruning and new-key clearing remain under the same write guard. [Shared address ownership](egress-01-cached-address-ownership.md) is still deferred. |

For an attempted HTTPDNS render/fetch/parse failure, successful configured
fallback wins; failed fallback preserves the primary error. A generation whose
preparation already failed runs its configured fallback without an attempted
primary fetch. Endpoint egress and the returned backend-address policy are
separate boundaries. See the original [prepared-runtime contract](../../skills/04-review/link-sys/regression-guards/httpdns-prepared-runtime.md),
[endpoint policy](../../skills/04-review/ssrf/regression-guards/httpdns-endpoint-egress.md)
and [native cache identity](../../skills/04-review/backends/regression-guards/externalname-dns-cache-identity-and-lifecycle.md).

Excluded from this bounded inventory: complete Controller and LinkSys publication/
revocation, OS resolver internals, Hickory pool/cache/hosts lifetimes, reqwest
networking internals, complete concurrency/cancellation analysis, and the separate
Gateway `util::net::resolve_domain` path. No deferred design is selected here and
no whole-callback, crate or goal completion is claimed.

Fresh existing selections passed Common DNS 71, Common egress 11, Gateway
DNS-cache 26 and Gateway egress adapter 3 tests. Workspace all-target check/Clippy,
formatting and agent-doc validation passed with existing warnings. These are
scoped regressions, not a new controlled DNS/HTTP transport experiment, full suite
or performance measurement. Runtime code and test fixtures are unchanged.

## WAF direct operator and action execution boundaries

Source revalidated at `ec24c988a`. Gateway `plugins/http/waf/mod.rs` invokes the
leaf transaction for request headers/body and response headers/windows. This
inventory follows the direct runtime dispatch in
`edgion-waf/src/seclang/transaction.rs`; cold syntax files named operator.rs and
action.rs are not the runtime evaluator. The CompiledOperatorOperand enum in
compiler.rs has 12 variants, all represented below.

| Operand variants | Direct execution and ownership |
| --- | --- |
| String, Integer | Static template operands borrow validated generation bytes; dynamic templates expand owned output under the member limit. String relations use slice comparisons or memmem search. Static integer operands are compiled values; dynamic operands and selected inputs use the shared signed parser. Template-shape preparation remains [waf-20](waf-20-prepared-template-shape.md). |
| IpNetworks | Parse one input IP, then search the compiled network slice in order. Same-family comparisons construct a prefix mask against the normalized base. Prepared masks remain [waf-03](waf-03-prepared-ip-network-masks.md). |
| ByteSet, ValidateUrlEncoding, ValidateUtf8Encoding, Unconditional | Inspect the borrowed input with the compiled four-word byte set, percent/hex validation, UTF-8 validation, or constant success. URL validation skips plain byte runs through memchr. These branches do not build owned operand copies. |
| DetectSqli, DetectXss | Call the bounded injection wrappers. SQLi returns an inline fingerprint; XSS returns a boolean. The enclosing stage admits detector input/work before calling them. This is not a complete SQL/HTML tokenizer or detector equivalence audit. |
| Regex, Matcher, CapturePhrase | Use generation-owned compiled programs and request-owned execution scratch. Regex Boolean/Capture plans select separate scratch slots; initialization is lazy, not runtime program compilation. Matcher dispatch preserves accelerated/reference modes, selected pattern ranges and the existing stage cache. CapturePhrase returns a match descriptor from its selected search mode. See [scratch budgeting](waf-01-prepared-matcher-scratch-budget.md), [VM initialization](waf-15-regex-vm-cache-initialization.md) and [prefix reuse](waf-17-prefix-matcher-scan-reuse.md). |

inspect_predicate_stage_with_hit_matcher checks detector admission and observer
charges before evaluation, then applies negation before capture extraction.
Only eligible positive capture operators extract values; negated success does
not create those captures. Capture ownership is selected by the existing observer
and builder, so direct dispatch is not proof of allocation-free matching. The
existing [capture-hit decision](waf-09-profile-capture-hit-index.md) remains open.
Transform/multiMatch output ownership remains a separate [decision](waf-07-multimatch-stage-ownership.md).

| Action/state boundary | Direct execution and retained constraints |
| --- | --- |
| apply_member_actions | Counts SetVar actions before expansion. Delete can borrow static key bytes; Assign/Add/Subtract expand owned keys. Arithmetic reads the staged value, uses a compiled constant or expanded delta, checks conversion/overflow and writes formatted output. Earlier staged writes remain visible to later actions. [Write-key ownership](waf-19-tx-write-key-ownership.md) remains deferred. |
| apply_ctl | ForceRequestBodyVariable changes staged policy; RequestBodyProcessor checks phase/body state before selecting the staged processor. RuleRemoveByTag and RuleRemoveTargetByTag visit the compiled tag index, charging each visit before insertion/deduplication. Sorted removed-unit and target-exclusion collections retain their limits and ordering. The suffix upper-bound optimization does not suppress duplicate visits. |
| TxState lookup and write | Lookup is case-insensitive and resolves bytes through the canonical owned/shared MatchValue boundary. Replacement retains the stored key and replaces its value; a new SetVar key is retained in the staged transaction. General capture writes can also use shared handles, so this is not an all-owned-TX claim. Delete and set update payload/retained accounting through the existing helpers. |
| validate_runtime_actions and finding_metadata | Msg/LogData expansion validates and meters macros without retaining expanded output. Finding metadata scans compiled member actions for severity and tag count; preparation remains [waf-02](waf-02-prepared-finding-metadata.md). Neither is a new hot-path tracing channel. |
| commit_unit_match | Runtime action validation and aggregate retained-byte admission precede publication of staged TX and capture-key state. This records the direct commit gate, not every chain search, replay or rollback path. |

Original contracts: [request-scoped controls](../../skills/04-review/waf/regression-guards/request-scoped-control-actions.md),
[fused inspection ownership and accounting](../../skills/04-review/waf/regression-guards/fused-json-inspection.md)
and [bounded injection operators](../../skills/04-review/waf/regression-guards/native-injection-operators-use-bounded-detection.md).
Deeper regex VM, matcher/search, injection scanners and transformation internals,
complete compiler/publication behavior and all branch/replay interactions remain
outside this bounded inventory. Existing deferred choices are neither selected
nor closed; whole-WAF, whole-callback and goal completion remain unproven.

Fresh WAF library verification passed 854 tests with six existing ignored.
Formatting, workspace all-target check/Clippy and agent-doc validation passed
with existing warnings. Runtime code and fixtures are unchanged; no full-workspace
suite, live Gateway integration or performance experiment ran.

## SQL tokenizer direct dispatch and ownership boundaries

Source revalidated at `b757137cb`. The preceding WAF operand inventory reaches
DetectSqli after stage input/work admission. `injection::detect_sqli` constructs
SqliState over borrowed input; detect conditionally fingerprints the original,
MySQL and quote contexts (up to five in the default wrapper path). Each fold
constructs SqliTokenizer::new with no custom lookup callback. reset clears the
public token projection; fold_tokens separately clears/resizes its workspace.
The wrapper returns an inline fingerprint and does not call fingerprint_string.

SqliTokenizer retains a borrowed input slice, scalar state, an inline current
Token and an optional borrowed lookup callback. Token contains scalar metadata
and a 32-byte array. Its assignment retains at most 31 payload bytes while parser
cursors can advance across the full input. Returned Token clones copy inline
data, not heap-owned payloads. SqliState's two SmallVec owners and their final
projection are a separate [storage decision](waf-14-sqli-token-storage.md); this
does not establish allocation-free whole detection, transactions or captures.

get_char_type indexes the pinned 256-entry CHAR_MAP. Default keyword lookup
normalizes into a 29-byte stack buffer, accepting each Unicode uppercase result
only when it is one ASCII scalar, then binary-searches the static table. The
optional with_lookup_fn hook is not installed by the production fold path.
This inspection does not verify arbitrary external callbacks or regenerate the
pinned data.

| Direct branch group | Inspected behavior and boundary |
| --- | --- |
| White, single-byte punctuation/operators, Other, Backslash | Advance scalar cursors and assign inline tokens; the special backslash-N pair becomes a Number. Whitespace dispatch is distinct from the explicit whitespace predicate used by other parser decisions. |
| Two-character operators | Check the special three-byte operator first, then a valid-UTF-8 two-byte table lookup, then colon/single-character fallback. No owned lookup key is constructed. |
| Hash, dash and slash comments | Preserve dialect-dependent dispatch and statistics, including the existing MySQL hash count. Line comments terminate only at LF. Block comments retain first closing-pair search, the pinned nested-search region and conditional-comment Evil classification. |
| Ordinary and simulated quotes, E/U/N/Q prefixes | Preserve escape precedence, prefix fallback and quote metadata. Simulated opening quotes keep a separate parser path. Q/NQ endings use the encoded high-byte delimiter but still advance end_pos + 2 after a match. These branches are not consolidated. |
| Binary/hex strings and numbers | Scan existing digit sets, retain closing-quote or word fallback, and preserve numeric prefix, decimal, exponent and suffix branches. No numeric value conversion or prepared numeric representation is introduced. |
| Words, variables, bracket words and backticks | Borrow input ranges and copy only the bounded token payload. Dot/backtick prefix keyword probes stay bounded by current.len. Variable counts preserve leading @ markers, initially quoted names delegate, and word/variable delimiter sets remain distinct. Bracket words use their first closing bracket; backticks retain their function-or-bareword classification. |
| Money and dollar strings | Numeric runs precede plain/tagged dollar dispatch, including the single-dot word fallback. Tags remain ASCII letters only. Closing searches use the first exact borrowed marker match; incomplete endings retain existing token metadata and full cursors. |

Recent search/predicate batches are recorded in INDEX. Arbitrary delimiter-table
preparation remains [WAF-21](waf-21-sqli-delimiter-classification.md). Neither that
choice nor WAF-14 is selected or closed by this inventory. Full folding cascade
and whitelist branch equivalence, generated-table/reference revalidation,
dependency machine-code/performance analysis and complete detector/callback
coverage remain outside this direct-dispatch inventory. This is no whole-WAF or
goal completion claim.

Fresh unchanged-production WAF library verification passed 867 tests with six
existing ignored. Offline parser/folding/verdict fixtures ran. Formatting,
workspace all-target check/Clippy and agent-doc validation passed with existing
warnings. No full-workspace suite, fresh upstream-checkout comparison, live
Gateway integration or benchmark ran.

## SQL folding, fingerprint and whitelist control boundaries

Source revalidated at `fc80d5fb3`. This extends the tokenizer inventory through
SqliState::fold_tokens, generate_fingerprint, blacklist dispatch and the direct
whitelist branches. It records current source control and ownership, not a proof
of equivalence to every historical C comment or all pinned-reference inputs.

| Stage | Inspected behavior and optimization boundary |
| --- | --- |
| Initial consumption and lookahead | The eight-slot workspace is reset. Initial Comment, LeftParenthesis, SqlType and unary tokens are skipped; every fetched token increments stats_tokens. Exhaustion here publishes tokenizer comment statistics and returns zero. Five-token special reductions inspect four pattern shapes, sometimes carrying slot five into slot one before continuing. Later comment tokens update last_comment; a non-comment clears that trailing-comment candidate. |
| Two-token folding | Ordered branches retain their distinct restart/backtrack behavior. Word merging uses a 32-byte stack array under a combined-length guard and the canonical lookup. Function/IN/LIKE reclassification, SQL type removal, collation, backslash, parentheses and brace branches are not interchangeable. LIKE/Collate fall through to three-token processing, not to later two-token arms. |
| Token text inspection | value_as_str borrows valid UTF-8 or returns the literal `<binary>`; it is used by runtime folding despite its display-oriented comment. cstrcasecmp uppercases ASCII bytes on the compared side and retains its length/NUL behavior. Raw-byte substitutions must not accidentally change invalid-UTF-8 collation or classification outcomes. Token clones still copy inline metadata and arrays. |
| Three-token folding | Numeric/operator/logic/variable reductions, casts, comma sequences, unary removal, dotted names and USER reclassification retain branch priority. Statistics increments are not uniform: the cast branch adds two. Comma/unary/value retreats pos by three, whereas comma/unary/function removes one; USER can fall through to advancing left. |
| Early return and final projection | Empty bareword after LeftBrace becomes Evil and copies through left + 2 before returning, bypassing ordinary final comment-stat publication. Normal completion caps left at five, applies the empty-unclosed-backtick comment rule, copies non-None tokens, publishes tokenizer statistics and appends last_comment only when left permits it. Return count and projected vector length are distinct quantities. |
| Fingerprint and blacklist | Token kinds fill the fixed eight-byte fingerprint with remaining bytes zeroed. Any Evil marker collapses it to X and replaces a nonempty finalized token vector with one Evil token. Blacklist lookup prefixes the short fingerprint with ASCII 0 in a six-byte stack buffer, then uses canonical keyword normalization/search. |
| Whitelist dispatch | UTF-8 fallback and trailing-NUL trimming produce one borrowed view reused by the two/three-token helpers. A trailing-comment fingerprint can trigger the whole-input case-sensitive sp_password check before branch dispatch. True here means the fingerprint remains classified as SQLi; the blacklist check precedes this path in detection. |
| Two-token whitelist | Union/statistics, hash comment, bareword/comment and number/comment cases retain their order before the remaining dash-comment/default branches. The numeric comment boundary reads input[token.len], not input[token.pos + token.len], and the actual whitespace comparison is unsigned <= 32. This records code rather than endorsing conflicting signed-byte prose. |
| Three-token whitelist | String/logic patterns depend on quote metadata and stats_tokens. The later Keyword check uses length >= 5 and the uppercase INTO byte prefix, not exact full INTO OUTFILE/INTO DUMPFILE phrase comparison. No simplification of those predicates was made. |

The repeated fingerprint-view conversion has been removed in its separate batch.
No additional allocation-removal claim follows from inline clones or stack word
merging. [WAF-14](waf-14-sqli-token-storage.md) retains the storage/projection choice;
[WAF-21](waf-21-sqli-delimiter-classification.md) retains arbitrary delimiter-table
preparation. This inventory selects neither alternative. Exhaustive reference
comparison, optimized-code/performance analysis, all adversarial fold sequences
and complete WAF/callback/goal coverage remain unproven.

Fresh unchanged-production WAF library verification passed 868 tests with six
existing ignored. Offline parser/folding/verdict fixtures ran. Formatting,
workspace all-target check/Clippy and agent-doc validation passed with existing
warnings. No full-workspace suite, fresh upstream-checkout comparison, live
Gateway integration or benchmark ran.

## XSS tokenizer, classification and decoder boundaries

Source revalidated at `a52b345b6`. DetectXss reaches injection::detect_xss after
stage input/work admission. The stateless detector visits DataState,
ValueNoQuote, ValueSingleQuote, ValueDoubleQuote and ValueBackQuote in order,
stopping on the first XSS verdict. ValueNoQuote initializes before_attribute_name,
not the unquoted-value state. XSS returns a verdict without SQLi-style capture.

Html5State borrows input and stores its fixed length, cursor, close-tag flag,
function-pointer state and current token metadata. set_token stores a borrowed
suffix plus a separate token_len; consumers slice that length, not the entire
suffix. No owned HTML token or decoded URL is built. The 23 state methods,
including EOF, are grouped below; this is direct source/state coverage, not
exhaustive transition-sequence equivalence.

| State group | Inspected behavior and preserved boundary |
| --- | --- |
| Data and EOF | Locate the next less-than byte or emit remaining DataText. Non-emitting transitions can delegate to the next state; EOF returns false. |
| Tag open/name/end/close/self-close | Dispatch markup, slash, question-mark and percent forms; alphabetic/NUL handling remains state-specific. Tag names retain NUL bytes in borrowed spans. TagClose, TagNameClose and TagNameSelfclose are distinct outputs, with different delimiter advancement and is_close updates. |
| Before/name/after attribute | h5_skip_white includes NUL while ordinary whitespace predicates do not. EOF is Option::None, distinct from every byte including 0xFF. Repeated slash handling uses the existing loop. Attribute-name scanning starts one byte after its initial byte and preserves delimiter-specific cursor/state behavior. |
| Before/value/after attribute value | Single, double and backquote states skip the opening quote only when pos > 0, preserving context starts. Quoted scans use the first matching delimiter; unquoted scans retain whitespace/greater-than boundaries. The latter can defer the closing-character token to a separate state. |
| Markup and doctype | Double dash selects comments, ASCII-insensitive DOCTYPE selects doctype, exact CDATA marker selects data, and other forms use bogus comments. Doctype tokens retain their source prefix and end at the first greater-than byte or EOF. |
| Comments and CDATA | Ordinary comment ending recognizes dash, optional NUL run, dash-or-bang, then greater-than, retaining its returned closing length. Bogus comments use a greater-than boundary; percent comments use the first adjacent percent/greater-than pair. CDATA uses its first exact ending triple. Unmatched paths retain their existing emitted token and EOF behavior. |

The detector clears attribute classification before every non-AttrValue token;
AttrName then sets the classification for a following value. Black and Style
attributes reject on that value, AttrUrl invokes URL prefix checks, AttrIndirect
reclassifies the value and rejects only Black, and consumed values clear the
classification. Doctype rejects immediately; tag names use blacklist lookup.
Tag/attribute normalization skips NUL, uppercases ASCII into a 64-byte stack array
and rejects excess non-NUL bytes. Tags retain their raw-length precheck and SVG/XSL
prefix rules. Attribute lookup retains XMLNS/XLINK, ON-event and other static
binary searches. These are bounded scratch operations, not heap normalization.

Production comment checks are inline in is_xss: backtick, conditional/XML prefixes
and exactly six-byte IMPORT/ENTITY comparisons. The unused is_dangerous_comment,
limited comparison, peek and skip_whitespace helpers are not production hot-path
coverage or optimization evidence.

URL handling first trims raw bytes <= 32 or >= 127, then probes DATA, VIEW-SOURCE,
VBSCRIPT and JAVA independently. Numeric entity decoding accepts decimal or hex
with optional semicolons. Invalid numeric starts and values above 0x1000FF return
ampersand with one byte consumed; named entities are not generally decoded.
A nondigit ends a valid numeric run before that byte, while a semicolon is
consumed. Empty input returns -1 with zero consumption. Prefix matching separately
skips leading decoded values <= 32, always skips NUL/LF, then uppercases ASCII
before casting to u8. These are prefix checks, not URL parsing or Unicode
normalization. The repeated decoding tradeoff remains [WAF-13](waf-13-xss-url-prefix-decoding.md).

No further runtime change or representation choice was made. The preserved input
cap and work model are resource accounting, not instruction/wall-clock bounds.
Full static-table/reference revalidation, adversarial state-sequence equivalence,
dependency/performance analysis and complete WAF/callback/goal coverage remain
outside this inventory.

Fresh unchanged-production WAF library verification passed 869 tests with six
existing ignored. Offline parser/folding/verdict fixtures ran. Formatting,
workspace all-target check/Clippy and agent-doc validation passed with existing
warnings. No full-workspace suite, fresh upstream-checkout comparison, live
Gateway integration or benchmark ran.

## Native phrase scan and backend boundaries

At source baseline b0eb720e3, followed WAF Matcher and CapturePhrase dispatch
through matcher.rs into native contiguous-input search and backend methods.
This extends the operator inventory with direct runtime evidence, not complete
compiler, public iterator or stream coverage.

| Boundary | Observed ownership and execution |
| --- | --- |
| PhraseGroup scan | Borrow input and reuse PatternSet; accelerated/reference scans map matching local IDs through the compiled pattern_ids array into the outer bitset. No per-hit payload copy occurs in this projection. |
| CapturePhrase | Borrow input, choose accelerated/reference find, and return a small pattern/start/end descriptor. The compiled matcher uses case-insensitive LeftmostLongest semantics. |
| Standard search | Check root output and minimum length, then advance folded bytes through the backend. Optional prefilter runs only at root; candidates are confirmed by transitions. Return the first output at the reached boundary. |
| Leftmost search | Track the best root/runtime match and bounded lookahead. Candidate handling distinguishes possible starts from exact packed results. Output ordering, match-kind tie selection and completion checks remain in the shared search owner. |
| Anchored search | Limit traversal by the maximum pattern length, accept only outputs whose start equals the requested start, and retain earliest/Standard versus leftmost selection. |
| Pattern-set scan | Validate input and scratch shape before clearing. Insert root outputs and accumulated local IDs; anchored scans restrict range and output starts. Reference execution bypasses optional acceleration. Results remain in caller-owned bounded storage. |
| Sparse backend | Binary-search transitions, follow failure links on misses and use an optional dense root table. OutputCursor visits state outputs and then output links. |
| Contiguous backend | Select dense table or sorted sparse-transition slice, then follow failure links as needed. OutputCursor reads compiled flat outputs and output links. |
| DFA backend | Map the byte to its prepared class and index the compiled transition row. OutputCursor retains the same flat-output/link shape; pattern lengths remain compiled metadata. |

Backend and OutputIter borrow immutable owners; the cursor stores only state and
output index. Auto selection is made during construction from DFA cell/memory
policy and then root fanout, not reselected per byte. Repeated enum matches in
source are a conditional [dispatch investigation](waf-23-phrase-backend-dispatch.md),
not measured machine-code overhead. The separate DFA-layout/SIMD tasks remain
conditional, and the dormant stream lane remains scoped by WAF-16.

Fresh unchanged-production WAF tests passed 871 cases with six existing ignored.
Formatting, workspace all-target check/Clippy and agent-doc validation passed
with existing warnings. No runtime code changed; no full-workspace suite, live
Gateway integration, code-generation experiment or benchmark ran.

## Native WAF transformation and output boundaries

At source baseline 00a9aaf90, inspected the shared transform.rs walker, ordinary
and multiMatch wrappers, all 20 apply_one variants and their direct helpers.
Transaction and structured collector call sites establish reachability; this is
not complete compiler, observer replay or Gateway transport coverage.

The walker reports existing scratch admission/capacity and the Initial checkpoint
before checking the initial output cap. Its multiMatch materializing wrapper
separately prechecks that cap before reserving the outer stage vector. Each step
reports StepInput, charges current bytes, and checks input work immediately only
in ordinary mode. Both modes charge result bytes and enforce work before
StepComplete; this ordering must not be flattened.

Input/First/Second identify the borrowed input or one of two reusable Vec owners.
Identity recognition covers None, lowercase without uppercase, and capped
marker-free RemoveNulls/HtmlEntityDecode/UrlDecodeUni. Other steps admit growth
including the old destination, clear/reserve output, report actual capacity and
apply the transform. Identity still emits checkpoints and logical charges.
Nonidentity lowercase is known changed; other outputs are compared with input.
Ordinary mode advances to produced scratch even for byte-equal output, whereas
multiMatch advances and emits a new ordinal only for changed output.

The ordinary wrapper moves the selected final Vec or returns borrowed input.
Ordinary multiMatch copies raw input and changed stages, finishes recipe validation,
then its transaction caller evaluates those stages until a match. Structured
collection instead borrows stage callbacks into its existing provisional machinery;
observer failures stop the walk, while semantic predicate outcomes must not end
later transformation validation. These are distinct ownership/error contracts.

| Transformation group | Direct byte behavior and retained boundaries |
| --- | --- |
| None, Lowercase, RemoveNulls, RemoveWhitespace | None appends in apply_one but is an identity in the walker. Lowercase affects ASCII uppercase only. Filters retain byte order; whitespace removal uses Latin-space bytes including 0x85 and 0xa0. |
| Base64Decode | Skip CR/LF, stop at the first other nonalphabet byte (including padding), decode complete quartets and residual two/three-digit groups. This is tolerant byte decoding, not strict whole-input validation. |
| CmdLine | Remove quote/backslash/caret bytes, coalesce listed whitespace/comma/semicolon separators, lowercase ASCII, and remove a pending output space before slash/open-parenthesis. Earlier output-limit failures still precede later removal. |
| CompressWhitespace | Collapse Latin-space runs into one ASCII space without trimming leading/trailing runs. Its byte class is broader than the ASCII-space helper used by CSS. |
| CssDecode | Consume up to six hex digits, map the low byte with the existing fullwidth adjustment, then optionally consume one ASCII-space byte. Bare escaped LF is removed; a terminal backslash produces no byte. |
| EscapeSeqDecode, JsDecode | Retain separate marker/simple/octal rules. Escape accepts x/X and up to three octal digits; JS accepts lowercase x/u and limits leading 4..7 octal runs to two consumed digits. Fixed hex fields validate/decode once; invalid sequences follow each decoder's original fallback. |
| UrlDecodeUni | Plus becomes space; percent u/U four-digit forms have priority over ordinary two-digit hex. Preserve low-byte/fullwidth mapping and literal malformed-percent fallback. |
| HtmlEntityDecode | Search the next semicolon or ampersand; only a semicolon-terminated recognized name/number decodes. Numeric parsing accepts decimal or x/X hex, skips leading zeros and checks overflow; invalid Unicode scalar values become the replacement character. |
| HexEncode, Length, Sha1 | Emit lowercase hex, decimal byte length or the raw 20-byte digest respectively. Decimal assembly uses bounded stack digits. Projected output uses checked 2x, fixed 40 and fixed 20 bounds. |
| NormalizePath, NormalizePathWin | Output doubles as the component stack; skip empty/dot components, reverse-scan and remove eligible preceding components, preserve unresolved relative parents and clamp absolute parents at root. Win additionally treats backslash as separator; retain trailing-separator rules. |
| RemoveCommentsChar, ReplaceComments | Marker removal retains its ordered marker recognition. Replacement scans slash-star to the first star-slash or EOF and emits one space, preserving unterminated-comment behavior. Neither is a general language parser. |
| Utf8ToUnicode | Copy ASCII and invalid/truncated UTF-8 bytes; valid scalar sequences emit percent-u followed by lowercase hex with at least four digits. No surrogate-pair conversion occurs. Projected output remains checked 6x input. |

append checks arithmetic and the whole segment limit before extending; push_byte
checks the current length against the limit before one byte. Their different
partial-output effects remain observable in helper regressions. Production
apply_one receives empty output with capped projected capacity already reserved.
No second body cache or response aggregation is introduced by these local owners.

[Marker-free decoder borrowing](waf-06-marker-free-decoder-borrowing.md),
[multiMatch ownership](waf-07-multimatch-stage-ownership.md) and
[final buffer transfer](waf-11-final-transform-buffer-transfer.md) remain deferred.
The original [fused inspection contract](../../skills/04-review/waf/regression-guards/fused-json-inspection.md)
owns provisional outcomes and aggregate admission; this inventory does not
establish complete replay equivalence or an allocation-free transaction.

Fresh unchanged-production WAF tests passed 873 cases with six existing ignored.
Formatting, workspace all-target check/Clippy and agent-doc validation passed
with existing warnings. No runtime code changed; no full-workspace suite, live
Gateway integration, external-reference comparison or benchmark ran.

## XML names, namespace scope and entity boundaries

At source baseline 2e19d4651, inspected xml_stream.rs name validation/storage,
attribute identity and namespace operations, closing-tag restoration, incremental
entity decoding and declaration validation. These serve active Full XML
inspection. This is not a complete lexical state-machine or replay inventory.

| Boundary | Direct behavior and ownership constraints |
| --- | --- |
| save_name and arena_copy | Charge name validation before copying into the live arena. Temporarily move name scratch while tracking held bytes/capacity, restore and clear it after the copy attempt, and refresh capacity even on failure. Arena copies admit structure and growth bounds before extension. |
| validate_ncname and validate_qname | Validate UTF-8 and XML name character classes per colon-separated component. Stop on the first component error; only afterward reject excess colons. Existing tested Character-versus-Syntax precedence is retained, not a newly delivered optimization. |
| attribute_name and attribute_done | Save the lexical name, check attribute count, recognize namespace declarations and enforce reserved prefix/URI restrictions. Namespace URI bytes are copied into the arena before binding publication. Attribute records retain spans and URI descriptors, not another payload owner. |
| prefix_local and prefix_uri | Split at the first colon using existing memchr. Resolve implicit xml, reject xmlns, treat unprefixed ordinary attributes as empty-namespace, and reverse-scan live bindings with a charge before each comparison. Missing nonempty prefixes fail Syntax; elements can use the default namespace. |
| close_start validation | Resolve the element first, sort/check lexical attribute duplicates, then resolve ordinary attribute namespaces. Reuse each charged QName split. Expanded sorting/checks run only when a prefixed ordinary attribute requires them. Late same-tag declarations remain visible. |
| attr_compare and sort_attrs | Charged in-place heap sorting compares declaration class, URI bytes and local names for expanded identity, or lexical names otherwise. Comparisons/swaps preserve their work-failure points; there is no unbounded temporary attribute map. |
| close_start lifetime | Empty elements reclaim arena/bindings to saved marks. Nonempty elements compact only new namespace bindings after the element name, truncate expired attribute identities, then reserve/push a frame. Attributes clear after completion; retained capacity is refreshed. |
| close_end | Validate the closing QName and charge exact lexical comparison with the last open frame. Only a matching name pops the frame and restores arena/binding marks; namespace URI equivalence alone does not close an element. |
| entity_byte | Track bounded named bytes or an incremental numeric value/count. Recognize only the five predefined names; numeric references require digits, checked arithmetic and XML-character validity at semicolon. Lowercase x selects hex. Decode into a four-byte local UTF-8 buffer, admit decoded bytes including pending text whitespace, then append through token_byte. Individual appends can still fail; this is not atomic output publication. |
| validate_declaration | Require separated quoted fields with version 1.0 first; allow optional UTF-8 encoding and yes/no standalone in their accepted order. Preserve XML-only whitespace, quote search, case-sensitive field names and case-insensitive encoding value. This is not general encoding support. |

The arena must outlive namespace/name use; synchronous value callback borrowing
does not authorize borrowing transient input for persistent bindings. Attribute
values are observed under lexical names in source order before start-tag identity
validation; canonical structured publication still owns final outcome ordering.
See the original [native XML contract](../../skills/04-review/waf/regression-guards/native-xml-inspection.md).
[Namespace lookup reuse](waf-18-xml-prefix-lookup-reuse.md) remains deferred, with
its charge/error ordering and scope-expiry obligations intact.

Existing tests cover QName error precedence, declaration grammar, namespace
shadowing/reserved bindings, duplicate identities, reclamation and chunk splits.
Fresh unchanged-production WAF tests passed 873 cases with six existing ignored;
formatting, workspace all-target check/Clippy and agent-doc validation passed
with existing warnings. No runtime changes, full-workspace suite, external XML
comparison, live Gateway integration or performance experiment accompanied this
inventory. Complete state-machine, callback and goal coverage remain unproven.

## XML byte admission, probes and lexical states

At source baseline 230016d10, inspected data, lookahead, admit_byte, character,
account_tag_byte, advance, emission and finish/feed control. This extends the
name/scope inventory through direct lexical execution, without proving every
parser interleaving or structured replay/transport behavior.

| Boundary | Direct execution and ordering |
| --- | --- |
| lookahead and admit_byte | Lookahead checks source/raw-token distance limits then charges work before inspection, without advancing source/raw counters. Actual admission separately advances source and raw counts under their existing ordered checks, then charges work and validates characters. Failure can leave different partial usage depending on the checkpoint. |
| Contiguous text/attribute probe | Probe only under the existing empty-token/CR/UTF-8 conditions. Stop on state-specific markers, with the stopper included in lookahead work. A successful terminated value borrows the input slice; text trims outer XML whitespace, while attributes do not. Emit and attribute_done can fail before delimiter admission. Failed probes are suppressed within the current chunk by probe_until. |
| CDATA probe | Search with bounded lookahead for the closing bracket/bracket/greater-than sequence. CR prevents the direct borrowed route. On success, admit value and closing delimiter before decoded accounting and emission, including empty values; incomplete candidates fall back to scalar state processing. |
| Scalar byte path | Admit and account the original byte before CR/CRLF normalization. A skipped LF after CR still receives admission/accounting. Incremental UTF-8 validation applies even to ignored markup. |
| Beginning, Text and Open | Recognize the optional initial BOM; enforce root-external text restrictions, pending trimmed whitespace and forbidden literal CDATA close in normal text. Open selects end tag, declaration, PI or start tag. Recursive advance transitions process the already admitted byte without admitting it again. |
| Tag and attribute states | StartName/Between/AttrName/Equals/Quote/Value/AfterValue/Slash and closing-name states retain whitespace, quoting and separator requirements. Scalar attribute completion temporarily moves token scratch while holding its heap charge, restores it after emission/declaration handling, and only then advances state on success. |
| Declaration, Comment and CDATA states | Fixed declaration discriminators admit only comment or CDATA openings; unsupported declarations fail. Comment closing and CDATA pending bracket counts are explicit states. CDATA is untrimmed; buffered emission includes empty values. |
| PI states | Validate the target as NCName. Reserved xml target handling is case/location sensitive; PI data requires separation unless the target closes immediately. Only XML declaration data is retained for declaration validation. Ignored PI/comment bytes still pass admission/character checks. |
| Entity and emission | Entity state delegates to the incremental decoder recorded above. prepare_emit checks context and charges event/summary/work; emit or emit_token calls the sink afterward. emit_token clears scratch/trailing state even when its callback path fails; no atomic emission guarantee is inferred. |
| finish and feed | Finish rejects incomplete UTF-8 before structural incompleteness, requires Text state, no open frames and exactly one root, then emits trailing token and marks completion. feed retains its first processing error. AlreadyComplete is returned separately after successful completion. |

[WAF-24](waf-24-xml-lookahead-batching.md) defers possible probe batching pending
measurement and an accounting design. Current processing already has bounded
linear failed-probe behavior. No scalar check, callback, scratch lifetime or
borrowing rule changed here. Original authority remains the
[native XML guard](../../skills/04-review/waf/regression-guards/native-xml-inspection.md).

Fresh unchanged-production WAF tests passed 873 cases with six existing ignored.
Formatting, workspace all-target check/Clippy and agent-doc validation passed
with existing warnings. No full-workspace suite, external XML comparison, live
Gateway integration, prototype or benchmark ran. Broader coverage remains open.

## Canonical certificate projection boundaries

Revalidated at `9a56ee913` in `tls/runtime/backend/cert_extractor.rs` and its
four production caller groups. This reconciles direct extraction coverage; it
is not a complete native X.509/BER dependency audit or live-handshake proof.

| Boundary | Current ownership and ordering |
| --- | --- |
| Handshake metadata | `tls_pingora::handshake_complete_callback` calls full extraction for exposed EdgionTls mTLS metadata or frontendValidation mTLS. The SSL entry point acquires an optional peer-certificate handle; missing certificate returns None. |
| Leaf mTLS policy | `mtls_verify_callback` acquires the leaf handle once. Missing certificate rejects first; nonempty SAN policy uses typed projection. SAN failure builds full metadata for redacted diagnostics before returning. A nonempty CN policy builds full metadata after SAN success. Leaf-depth and terminal-audit gates remain owned by the callback. |
| HeaderCertAuth | Header parsing and configured CA checks precede the same full certificate projection. Base64 DER and URL-encoded PEM remain separate input paths; extraction does not replace verification. |
| Backend SAN hook | Nonempty prepared policy installs a hook calling display SAN extraction. Missing peer certificate produces an empty vector; canonical policy matching and the redacted panic boundary remain outside the extractor. The backend-independent rustls verifier has its own DER path, covered separately. |
| Subject, CN and fingerprint | Traverse native subject entries once, retaining order and separators. Mark the first CN before UTF-8 conversion, so an invalid first CN is not replaced by a later one. Native string conversion retains its C-string NUL behavior. SHA-256 digest renders lowercase colon-separated bytes; digest failure returns the existing unknown text. |
| Ordinary SAN projection | Enumerate native SAN entries, projecting DNS/IP/email/URI in existing order. Invalid IP byte lengths are skipped. Text borrows the native entry until synchronous projection; IP stays typed until formatting. Display and PresentedSan each own their returned values, without first building the other projection. |
| OtherName pass | Serialize certificate DER, parse X509Certificate, and append supported OtherNames after ordinary SANs. Missing/failed DER or SAN parsing returns without deleting preceding ordinary output. The certificate parser's returned remainder is ignored; this is distinct from the stricter BER-value remainder checks. |
| BER wrappers and strings | Require a complete outer BER object and context-specific constructed tag zero. Unwrap once and require Universal content; the Unknown-wrapper branch also rejects an inner parse remainder. Supported text borrows input through projection; boolean/NULL borrow static text. Numeric/OID/BMP/Universal conversions own output. Unsupported values are skipped. |
| INTEGER and wide strings | INTEGER admits only nonempty payloads of at most 64 bytes, copies to bounded scratch, resolves signed two's complement, and emits decimal through bounded digit scratch. This is not a bound on all DER/string parsing. BMP uses big-endian UTF-16 units, accepting valid surrogate pairs; Universal uses four-byte scalar validation. Embedded NULs remain in these OtherName strings. |

The borrowed OtherName OID is formatted during projection; its dependency Display
may allocate internally. Borrowed text and parser/native owners do not escape
extraction. Earlier subject, BMP, IP, OtherName text/OID and decimal-scratch
optimizations are existing behavior, not changes delivered by this inventory.

Existing decisions remain [backend SAN inspection](tls-05-backend-san-inspection.md),
[minimal CN projection](tls-06-mtls-cn-projection.md), and
[prepared SAN policy](tls-01-prepared-san-policy.md). Omitting the DER pass or
sharing projections would need their ordering, failure, identity-type and
lifetime decisions; no new implementation is selected here.

Fresh targeted tests passed 18 extractor, 14 mTLS callback and 20 HeaderCertAuth
cases. Formatting, workspace all-target check/Clippy and agent-doc validation
passed with existing warnings. No runtime/test edit, full-workspace suite, live
TLS integration or benchmark accompanied this inventory.

## DSL scalar utility direct execution boundaries

Source baseline: `805ac19e3`. This inventory covers exactly 23 explicit utility
arms in `plugins/http/dsl/lang/builtins.rs::call_builtin`, not the entire DSL.
Both DslPlugin request and upstream-response entries reach execute_safe; the
response entry retains its multi-thread Tokio synchronous bridge. Generic calls
compile arguments in source order, then dispatch CallBuiltin. The VM charges
an opcode step and builtin call before synchronous dispatch, then pushes the
result with stack-depth admission and container-shape inspection where applicable.
These counters do not meter each byte of work inside a builtin.

RuntimeString delegates to Value::try_into_string: existing Arc text is copied,
scalars are rendered, List/Map use their marker forms, and Body/Xml conversion
fails. The output limit is not a universal input or temporary-allocation bound.
Direct source inspection establishes the following behavior, not a new policy
acceptance or exhaustive dependency audit.

| Explicit arms | Direct work and retained boundary |
| --- | --- |
| Len | Counts Unicode scalars for Str, entries for List/Map, zero for nil; other values return TypeError. No temporary string conversion. |
| Substr | Validates end then start, clamps negative indices to zero, and finds scalar-index byte boundaries in one pass. Checks selected bytes and retains the original Arc for a full span; partial output owns its Arc. Nil/reversed ranges yield empty text. |
| ToStr, ToInt | ToStr preserves an existing Arc after the byte check; other values use RuntimeString and independent output ownership. ToInt parses Str as i64, retains Int, and returns nil otherwise, including failed parsing. |
| ToUpper, ToLower, Trim | Case conversion uses standard Unicode conversion, allocates its result, then checks output bytes. Trim borrows the trimmed slice, checks it, and converts the existing input owner when unchanged or copies the trimmed slice into Arc. No ASCII-only policy substitution. |
| Base64Encode, HexEncode | After coercion, exact padded-Base64 or doubled-byte length admits the existing encoder before output allocation. Accepted output becomes an independent Arc. Defensive overflow handling is not evidence of reachable oversized Rust strings. |
| Base64Decode, HexDecode | Dependency decode failure returns nil. Successful bytes use lossy UTF-8, then final-byte validation and independent Arc creation. Decode and possible lossy replacement storage precede that check; encoded input length is not an exact decoded-text admission test. |
| UrlEncode, UrlDecode | Encoding preserves the unreserved set and uppercase escapes, checking after a complete Unicode scalar; initial capacity is input length. Decoding has a marker-free shortcut, otherwise strict two-ASCII-hex decoding, literal invalid-percent rescanning, plus-to-space and whole-buffer lossy UTF-8. Neither route authorizes substitution of the resource decoder's malformed-input semantics. |
| Sha256, Md5, HmacSha256 | Retain argument/coercion and crypto order. Fixed digest hex text uses stack buffers, canonical byte checking and independent Arc output. HMAC pops data before key. Crypto implementation internals are outside this inventory. |
| Split | Counts full non-overlapping matches or Unicode scalars plus boundary elements, rejects exact ListTooLong before result construction, then reserves and splits into Arc elements. No per-element string-limit check; the two-pass scan decision remains deferred. |
| Range | Validates end then start as Int or nil, checks subtraction, clamps count to nonnegative/min(max_list_len, 1000), and checks the end bound. A reused numeric String feeds independent Arc elements. No per-element string-limit check. |
| Join | Converts delimiter before validating List input; checked arithmetic admits the exact combined byte length before allocation. Overflow reports usize::MAX. Copies items/separators into one reserved String and then independent Arc. |
| IpInCidr | Converts CIDR then IP, parses IP first, borrows network/prefix with split_once, then parses network and u8 prefix. Malformed/mixed-family/out-of-range input returns false; same-family masks preserve zero/full-prefix behavior. This is the scalar DSL predicate, not a business ACL policy owner. |
| TimeNow, Uuid | TimeNow reads SystemTime, returns epoch seconds and zero for pre-epoch error. UUID v4 uses stack text encoding followed by byte validation and Arc output. OS clock/RNG internals and their failure policies are not reviewed here. |
| TimeFormat | Converts and gates the format at 256 bytes before timestamp handling; nil/out-of-range skips formatting. Direct fallible Chrono writing preserves ApiError before successful-output byte checking. Literal-format parsing still happens at execution. |

The rows enumerate 23 arms (1+1+2+3+2+2+2+3+1+1+1+1+2+1). Logging,
session access/mutation, JSON/XML/Body, Redis and outbound HTTP branches are not
included. Regex search/replacement and other specialized opcodes retain their
separate compiler/validator/runtime boundaries. This does not establish complete
VM, standard-library, codec, crypto or plugin coverage.

Remaining ownership and representation decisions are [DSL-01](dsl-01-string-consumer-ownership.md),
[DSL-11](dsl-11-url-decoder-consolidation.md), [DSL-12](dsl-12-split-scan-reuse.md)
and [DSL-13](dsl-13-time-format-preparation.md). No new runtime proposal or
performance result is implied by consolidating the direct source inventory.

## DSL VM opcode direct dispatch inventory

Source baseline: `c0db721ca`. The 52 variants in `edgion-dsl/src/bytecode.rs`
match the 52 direct arms in Gateway `plugins/http/dsl/lang/vm.rs::execute`.
The table lists every variant once. It covers direct dispatch and inline work,
not all delegated scanner, engine, transport or storage implementations.

Execution charges tick_step before checking whether pc has reached the end;
implicit GoodNext therefore also requires a remaining step. Fetch increments pc
before dispatch, so relative jumps use that advanced position. Ordinary result
push first checks stack depth, then inspects Map/List shape, then stores the
value. It is not a universal string-byte check. These are observed execution
boundaries; the separately validated bytecode contract remains required.

| Opcodes | Direct work and delegated boundary |
| --- | --- |
| LoadConst, LoadNil, LoadTrue, LoadFalse, Pop, GetLocal, SetLocal | Constants use typed pool access and cached Arc text; non-value constants fail. Local reads inspect/charge then clone, with nil for a missing slot; writes move into an admitted slot or fail. Push and Pop retain their existing admission/underflow behavior. |
| Add, Sub, Mul, Div, Neg | Integer arithmetic is checked, with explicit division-by-zero and signed-overflow handling. Add also performs typed string concatenation, exact byte admission, empty-operand Arc reuse and owned-prefix reuse. Mixed coercion remains canonical; no opcode cost is inferred from the byte cap. |
| Equal, NotEqual | Body/Str in either orientation delegates to the canonical body text predicate. Other pairs inspect both shapes and precharge comparison before Value equality; NotEqual negates the result. This is not a blanket special case for all Body pairs. |
| Less, Greater, LessEqual, GreaterEqual, Not | Ordering borrows matching Int/Int or Str/Str and otherwise errors; it does not enter the equality shape/comparison-charge path. Not applies canonical truthiness. The difference is recorded, not silently unified or accepted as a complete work model. |
| StartsWith, EndsWith, Contains | Str/Str borrows both operands for string search; Body/Str constructs an owned text operation for DslBodyOps. Other pairs yield false. Body scanner ownership is distinct from scalar borrowing. |
| Matches | Looks up the prepared streaming DFA before branching on the input. Str feeds and finishes a StreamingRegexScanner; Body delegates through the lossy body scanner; other types return false. This is not the regex::Regex cache used by find/replace. |
| TextLen, TextIsEmpty | TextLen counts Str scalars or delegates Body char counting, then checked-converts to Int; unsupported types error. TextIsEmpty checks Str directly or delegates Body inspection, with false for other types. |
| BodyByteLen, BodyRead, BodySha256, BodyMd5, BodyHmacSha256 | Validate operands in the existing pop order and delegate to DslBodyOps. Read checks nonnegative max bytes and offset before Body admission. HMAC requires an exact Str key. Length conversion and result push remain explicit; storage/scan limits belong to the delegated owner. |
| JsonGet, JsonSelect | Use prepared path/selection entries. JsonGet scans owned Str or Body and maps missing to nil; JsonSelect requires Body, runs the shared selection scan and projects named captures into a Map. TooLong maps to limit-plus-one evidence. Scanner parsing, capture ownership and aggregate bounds retain their separate contracts. |
| BodyClear, BodyAppend, BodyPrepend, BodyRemove, BodyReplace | Validate Body plus exact string or nonnegative range arguments, delegate mutation, then push nil. Replace rejects empty search after operand validation. These arms authorize no new body cache or response aggregation. |
| RegexFind, RegexReplace | Find uses prepared regex::Regex only for Str, preserves full-span Arc ownership and checks result bytes; other types yield nil. Replace coerces replacement before testing input type, then uses the common bounded replacement helper. No-match retains the source Arc but still checks its byte length. Helper errors retain distinct runtime mapping. |
| Jump, JumpIfFalse, LoopInit, LoopBack, LoopEnd | Jumps validate target bounds; conditional jumps pop canonical truthiness. LoopInit adds a counter, LoopBack increments/checks it before validating the target, and LoopEnd pops it. Compiler/validator loop structure remains a separate requirement. |
| ListLen, IndexGet, MakeMap | ListLen returns count or zero. IndexGet moves a List element or selected Map value from the consumed container; Map extraction retains conservative inspection/copy charges. MakeMap checks size and stack suffix, preflights raw pairs, then drains into a Map and pushes it. Pre-dedup and final shape checks differ. |
| CallBuiltin | Charges tick_call before selecting JSON, XML, HTTP, Redis, Body or synchronous dispatch, then admits the result. Specialized handler internals are not covered by this dispatch row. |
| ReturnNext, ReturnDeny, ReturnTerminate, ReturnGrpcError | Next returns GoodNext. Deny validates final HTTP status with fallback 500 and owns a non-nil coerced body. Terminate returns an HTTP 502 result. gRPC validates codes 0 through 16 with fallback 13 and owns a non-nil coerced message. These are returned decisions; downstream writes belong to plugin/runtime handling. |

The row counts are 7+5+2+5+3+1+2+5+2+5+2+5+3+1+4 = 52.
Request and upstream-response plugin entries reach execute_safe, with the
existing synchronous-to-async bridge on the response entry. BodyAccess errors
receive the dedicated fixed 500, fixed diagnostic and failure outcome before
ordinary error-policy handling. Other runtime errors and caught unwinding
panics receive fixed classifications and configured Ignore/Deny/DenyWith
handling. This boundary does not catch process aborts or prove preemption of
long synchronous operations.

Existing decisions include [DSL-01](dsl-01-string-consumer-ownership.md) for
coercion/body pattern ownership, [DSL-05](dsl-05-json-select-capture-transfer.md)
for capture transfer, [DSL-08](dsl-08-list-loop-source-ownership.md) for loop
copies and [DSL-14](dsl-14-value-shape-reuse.md) for repeated shape metadata.
The [scalar utility inventory](#dsl-scalar-utility-direct-execution-boundaries)
adds direct coverage for its named builtin subset. No new runtime optimization,
complete DSL coverage, dependency coverage or performance result is claimed.

## L4 route selection and backend admission boundaries

Revalidated at `94cc60835`. This follows TCP connection and UDP new-session
selection and TLS SNI selection through the Service discovery handoff. TLS
passthrough is enabled; `routes/tls/proxy.rs` is retained termination code, not an
enabled TLSRoute product path. Controller `listener_has_unsupported_tls_mode`
rejects explicit TLS/Terminate with UnsupportedValue. Inspecting shared code does
not relax that gate or establish live termination support.

| Boundary | Current source evidence | Ownership and remaining limit |
| --- | --- | --- |
| TCP/UDP table construction | `tcp_route_table.rs` and `udp_route_table.rs::from_units` collect cached unit Arcs and place the minimum resource key first. Other entries remain for ownership/counts without a full sort. | Key construction and minimum selection are cold rebuild work. Units retain their route-owned backend selector across unrelated table rebuilds. |
| TCP/UDP connection lookup | Both `select_route_unit` helpers load the manager table and clone only its first unit. No SNI or path matching runs. | The table guard ends at the helper return; the selected unit survives admission and transport work. Removing that selected Arc is not a temporary-string optimization. |
| TLS accepted attachments | `TlsRouteUnit::from_route` validates names and pairs accepted schema indices with exact logical parent identities, preserving authored parent order. | Hostname lists remain in the immutable schema owner; there is no second runtime copy. Route schema and compiled backend generation cannot be independently replaced. |
| TLS table construction | `TlsRouteTable::with_units` groups authored patterns by accepted intersections on the physical listener key, retains attachment indices and sorts candidates by timestamp/namespace/name. | Runners-up remain. Unchanged candidate groups and hostname topology can be reused; this is construction evidence, not a complete publication-interleaving proof. |
| TLS query and candidates | `select_by` normalizes SNI once, visits canonical matching groups and tests effective names plus logical owner on the same attachment. An ineligible candidate can yield to a runner-up or broader Route group. | Lowercase SNI borrows; uppercase normalization owns its lowercase result. `radix_hostname_matches` is a pure comparison on validated patterns, not repeated parsing. Listener specificity/admission remains authoritative. |
| TLS selected policy | `select_tls_route` uses the supplied publication's listener index and a separately loaded route table, then returns selected unit/owner and optional available scope. Both proxy implementations release the publication before plugin work. | Missing selected policy is terminal; it does not restart hostname iteration. The two snapshots are not claimed to be one atomic cross-store generation. |
| Stream admission | TCP/UDP and TLS resolve the selected stream resource before awaiting its checks; whole publication/scope handles are released first. | Connection/request-owned identity and selected executable resources remain owned. Denial does not trigger route or backend fallback. |
| L4 BackendRef selection | `CompiledL4Backends` prepares first-rule slots, targets and weights once; BackendSelector selects an index and returns a borrowed SelectedL4Backend from that sealed generation. | Denied and malformed slots retain their selected traffic share. Canonical Service namespace/name keys and port sections are prepared before the hot path. |
| Authorization and policy | `SelectedL4Backend::service` checks denial before kind validity. `resolve_l4_policy_backend` admits live policy ancestors before invoking discovery through `with_service`. | A refusal invokes no resolver closure. Live policy admission is not a promise to pin the later endpoint/policy state atomically; preserve its existing ownership boundary. |
| Discovery and retry | TCP/UDP call the canonical Service selector. The shared TLS attempt helper returns terminal on missing/denied/invalid selection and distinguishes unavailable resolution for its bounded retry loop. | Retrying an unavailable endpoint does not authorize satisfying a denied backend share with a sibling. Endpoint strategy internals have their [separate inventory](#service-endpoint-selector-dependency-boundaries). |
| Existing UDP sessions | The fast path reuses the session's pinned upstream; route/plugin/backend selection belongs to new sessions. | Rechecking every packet or tearing down sessions after a route update would change the hot-reload contract. |

No additional behavior-preserving edit was selected in this inspection. Existing
[Listener specificity](routing-02-listener-specificity-index.md),
[hostname content representation](routing-03-hostname-content-index.md) and
[L4 peer address](l4-01-structured-peer-address.md) decisions remain unselected.
Certificate/SAN matching is a separate mechanism from Route hostname matching.
Remaining publication races, complete policy/discovery interleavings, relay and
socket/dependency internals are not closed by this direct selection inventory.

Fresh unchanged-code tests passed TCP 29, UDP 61, TLS 85, compiled-backend runtime
14, backend policy 10 and canonical leaf hostname 22 cases. These 221 cases
support the named source boundaries; they are not live connection, hot-reload or
whole-repository verification. No new runtime code or performance benchmark was
introduced.

## AI dispatch ownership refresh after local optimizations

Revalidated at `240868570`. This section reconciles the named direct boundaries
with the implementation changes since the older AI retry/admission inventories.
It is not completion of the reachable DNS, Redis, TLS, body-parser or transport
graph. Paths below are relative to `edgion-gateway/src/` unless stated otherwise.

| Callback or direct owner | Current work and ownership boundary |
| --- | --- |
| `pg_upstream_peer` -> `ai/http/attempt::select_peer` | Drains pending settlement, prepares a target, admits it, checks deadlines after admission and endpoint resolution, then commits context only after successful peer construction. Target and selected generations survive those awaits. Endpoint failure advances the backend scope; credential and dependency refusal follow their existing scheduler scopes. |
| `ai/runtime/dispatch::select_target` | Uses one lazy exact-capacity local Vec per synchronous call. Static candidate fields are initialized once; every backend scan refreshes exclusion bits and adaptive snapshots in declaration order. No projection survives the call or model re-resolution. Direct credential/stopped paths retain lazy allocation behavior. |
| `prepare_ready_attempt` and `ai/connectors/provider` | Restore the post-rule header/deferred baseline before candidate preparation; preserve body-generation checks and canonical body ownership. Native Anthropic version observation borrows the winning deferred HeaderValue or inspects live cardinality with one lookup. Profile/RemoteIp rejection remains earlier, and final credential injection/expiry/Connection validation remains later. |
| `AiAttemptResponseProcessor` construction | One boxed-child pointer is inline in the existing SmallVec dependency; child Boxes remain and multiple pointers still spill. Installation predicates/order and all callback loops remain intact. The wrapper still owns usage and EOS, observes raw encoding before child header changes, short-circuits terminated body processing and forwards cleanup. It is not response aggregation or a new owner. |
| `PreparedAiAttempt::admission_input` -> `AiAttemptRuntimeState::admit` | Projection retains the selected policy generation. Admission transfers its owned breaker Arc into the permit, then admits the outlier/probe, then reserves optional Redis capacity. Local RAII release is separate from remote reservation settlement. Credential probe ownership remains [deferred](outlier-02-credential-probe-owner.md). |
| `pg_upstream_request_filter` -> `mark_ai_dispatch_started` | Dispatch accounting begins at the existing upstream-write boundary, after final header checks. Peer preparation and admission do not become dispatch attempts. This refresh changes no retry-budget sample or scheduler transition. |
| `pg_upstream_response_filter` -> `ai/http/attempt::on_raw_response` | Raw AI status handling applies 429 cooldown, records the response and evaluates retry using live transport/deadline facts. `ai/http/response` preserves Retry-After precedence and delays reset parsing until fallback is needed. Common `time::parse_rate_limit_reset` skips impossible duration parsing for date-shaped hints while keeping epoch precedence and RFC3339 validation. |
| `pg_fail_to_connect` / `pg_error_while_proxy` -> AI failure adapters | Preserve relay/head retry vetoes, predispatch release versus dispatched failure accounting, and exactly-once status-retry transition. These adapters still delegate scheduler and settlement policy to their existing owners. |
| `try_ai_backend_peer` / `resolve_ai_endpoint` | Structural inspection only in this refresh: resolve through canonical DNS, filter all addresses through canonical egress, select the first survivor, retain selected policy/TLS ownership, configure timeouts and record the resulting peer. No new DNS, TLS, network or cancellation experiment is claimed. |

Existing decisions are not reopened by this reconciliation: full baseline restore
[retry-03](retry-03-ai-request-baseline-rebuild.md), optional reservation projection
[redis-08](redis-08-optional-credential-reservation-input.md), and first-address DNS
[dns-02](dns-02-ai-first-address-projection.md). Full address filtering preserves
rejection observations after the selected address. A Vec expression alone does
not establish an extra allocation; retained generations across awaits likewise
do not establish an avoidable clone. No new runtime edit or duplicate task was
selected for this documentation batch.

Fresh verification passed 89 tests: dispatch 39, attempt runtime 26, provider seven,
response seven, breaker three and Common time seven. These are current direct-owner
checks; the HTTP attempt adapter, live peer/DNS/TLS/Redis execution and every
reachable child implementation are not newly tested by that set. Historical
verification in earlier sections remains historical evidence. No full workspace
test suite or performance benchmark ran.
Formatting, workspace all-target check/Clippy, agent-doc validation and diff
hygiene passed with existing unrelated warnings.

## AI request parsing and conversion ownership refresh

Revalidated at `7248b9bce`. This section reconciles the named request preparation
and conversion boundaries after recent local optimizations. It extends the older
direct-entry inventory; it does not close the complete callback dependency graph.
Source paths below are relative to `edgion-gateway/src/`.

| Entry or owner | Current parsing and lifetime boundary |
| --- | --- |
| `routes/http/proxy_http/pg_request_filter` -> plugin runtime -> `ai/http/proxy::AiProxy` | Ordered request-plugin dispatch invokes AiProxy through the existing RequestFilter interface. Its global-store wrapper supplies per-invocation entropy. Route/backend capture and admission remain separate from protocol parsing. |
| AiProxy body capture | Reads through the existing request-body interface, checks capture generation changes and any settled guard generation, then canonicalizes captured bytes. Configured quota admission can await; the final generation check still precedes plan publication. This is not an atomic transaction covering every body consumer. |
| `protocols/openai/request::prepare` | One top-level Fields parse supplies owned Chat metadata and optional usage injection. Metadata errors precede injection. Non-streaming requests skip injection work; streaming options still receive their own nested parse when needed. No borrowed parsed state crosses an await or survives a body replacement. |
| `protocols/openai/request::responses_metadata` | Selectively captures model, streaming, output bound and continuation facts from validated native UTF-8/JSON. The canonical Responses body remains unchanged; this path adds no Chat usage injection or conversation tree. |
| `protocols/anthropic/request::metadata` | Selective native fields and bounded nested capability inspection determine binding/composite flags. Plain kind discriminators borrow raw text; escaped kinds remain owned. Existing item/depth bounds and error wrapping remain. Native Messages is distinct from Chat conversion. |
| `protocols/request::decode_model` and native rewrite helpers | The shared decoder returns borrowed plain or owned escaped model text under the original encoded/decoded bounds. All metadata owners explicitly obtain a String. Comparison-only rewrites can borrow, while changed-model patching retains the canonical span/patch helpers and original unrelated bytes. |
| Logical model routing and quota downgrade | Local rewriting parses the current private canonical body; changed bytes update the mutation flag and, when already enabled, input-character accounting. Re-resolving candidates and logical quota identity remains separate from provider-model ownership. |
| `runtime/dispatch::prepare_ready_attempt` native branches | Matching provider/logical model names retain the plan's existing Bytes. Other selected names invoke the appropriate native rewrite on the current plan body. The shortcut does not mean every retry or rewrite avoids parsing. Body-generation publication and provider headers retain their later boundaries. |
| `conversion/chat_to_anthropic/request::prepare_anthropic_request` | Each selected conversion preparation receives a fresh owned input Value from dispatch. Validation keys borrow it; completed output arrays/argument objects move into parents. Remaining source-to-output copies are the [request-tree ownership decision](conversion-01-request-tree-ownership.md). Usage-option validation now reads its sole required key once. |
| AiProxy local model listing | GET model listing resolves visibility independently from POST preparation. The encoder serializes typed model entries with moved IDs, retaining the collection and final byte buffer. It is a local response, not upstream response aggregation or a reusable model-list cache. |

Native Chat's selective stream/output-bound interpretation is intentionally
separate from explicit conversion's strict compatibility checks. Non-true Chat
stream values are not a new validation error, and non-u64 output fields are not
silently made strict by the parsing refactor. Native Responses and Messages keep
their own required/nullable field contracts. Shared UTF-8 admission does not
replace each protocol's syntax, duplication or capability checks.

Converter BoundedWriter enforces serialized output bytes while writing. It does
not establish an input/output DOM heap bound or inherit the unary response
converter's memory reservation. No cross-attempt parse cache, new body owner,
response aggregation or parser-wide allocation guarantee is introduced.
Candidate provider-model ownership remains [deferred](routing-09-candidate-provider-model-ownership.md);
header baseline rebuilding remains [separate](retry-03-ai-request-baseline-rebuild.md).

Fresh unchanged-source verification passed six shared request, nine OpenAI request,
ten Messages request, five JSON span, 26 proxy, 19 request-converter and 39 dispatch
tests (114 total). These check the named parsing/preparation contracts and existing
local fixtures. They do not newly verify live providers, Redis/DNS/TLS execution,
body-capture internals or all publication/cancellation interleavings. No new runtime
edit, test, prototype, full suite or performance benchmark was introduced here.
Formatting, workspace all-target check/Clippy, agent-doc validation and diff
hygiene passed with existing unrelated warnings.

## Bounded unary conversion ownership refresh

Inspected at `8123a3e60`. This extends the earlier direct response-head and
processor-entry inventories with the inner unary parsing, serialization and
reservation owners. It concerns explicitly selected non-streaming
Chat-to-Anthropic conversion, not native AI observation or general response storage.

| Boundary | Current source and ownership |
| --- | --- |
| Startup and admission | `ai/config.rs` resolves limits and constructs one OnceLock runtime with a process-wide UnaryBudget. `runtime/dispatch.rs` retires the prior prepared processor/context owner before acquiring a nonblocking reservation for selected unary conversion; conflicts, missing runtime and admission failure remain local failures. |
| Attempt publication | Dispatch shares UnaryHead with the translator and moves the prepared context owner at commit. These are independent ownership branches, not duplicate copies of the response body. |
| Final response metadata | `head.rs` independently revalidates final-onion eligibility and resolves absolute Hold deadlines. `pg_response_filter.rs` compacts final headers after reconciliation; the upstream-trailer hook compacts after processor trailer dispatch. Fresh name/value byte reconstruction detaches original backing and capacity. Existing head-plan precedence and deadline details remain in the earlier inventory. |
| Receive and preflight | `process_non_streaming` requires its unary owner, checks expiry, appends under the input cap and waits for EOS. `translate_unary` runs canonical selective preflight with source, token, decoded, event, depth, retained, work, key, number and node bounds before creating the Value DOM. Parser scratch ends before DOM construction; the two passes have different responsibilities. |
| Validation and accounting | The converter validates message identity, role, every supported block, stop reason and usage before output publication. Checked input/cache/output totals reject overflow. Valid provider usage is published before later serialization failure and retained through cleanup, as required by the original accounting knowledge. |
| Text ownership | `append_unary_text` borrows the first nonempty validated fragment, ignores empty fragments after the cap check and promotes through canonical bounded appends only when concatenation is required. The DOM already remains alive through synchronous serialization. Empty text with tools produces null; empty text without tools remains an empty string. |
| Tool arguments and final output | IDs and names borrow the DOM. Each tool input is serialized under the remaining aggregate argument cap into a LimitedWriter; String::from_utf8 consumes that Vec without another byte copy. The final borrowed Serialize view embeds that JSON as a string and writes under the final output cap. JSON-as-string is a wire requirement; this specific intermediate String is not proven unavoidable. The response writer has no unread overflow flag, while the request writer retains its separately used flag. |
| Output lease | Successful conversion wraps the output Vec with an Arc reservation in `budget::OutputOwner` and releases the held head. Bytes clones and slices share output ownership. Dropping the processor's local owner does not release the slot while context or output owners remain. |
| Terminal cleanup | The translator's release/finalize hooks discard input, drop its unary owner and seal processing. `ai/http/mod.rs::finalize` invokes driver or builder cleanup and clears the context owner before accounting finalization. This source sequence does not establish that every suspended callback or transport task has completed. |

The retained-output budget test explicitly exercises both release orders and
Bytes clones/slices. Limits tests retain conservative reservation formulas;
response tests retain exact conversion, malformed/duplicate input, usage after
output failure, adversarial allocation and concurrent output-owner probes.
Those probes are not process-RSS bounds, allocator proofs or live transport tests.

[Retry-After storage](../../skills/04-review/security/regression-guards/anthropic-translated-responses-rebuild-headers.md#retry-after-ownership-decision) was declined after value review: moving one header may preserve a larger shared
receive allocation without a demonstrated net benefit. Header
compaction is deliberately a different ownership boundary. The
[stream-event input](conversion-02-stream-event-input.md),
[stream output envelope](sse-03-canonical-event-envelope.md) and
[request tree](conversion-01-request-tree-ownership.md) decisions remain separate.
No additional optimization is selected by this inventory. Canonical parser,
serde/allocator internals, complete fork Hold/transport interleavings and live
cancellation/delivery behavior retain their existing review boundaries.

Fresh unchanged-source verification passed 34 response-converter, three limits,
one budget, seven unary-head, 39 dispatch and eight proxy response-head tests:
92 total. No runtime edit, new test, full suite, live-provider test or performance
benchmark was introduced by this reconciliation.

The current contract is documented in
`skills/01-architecture/02-gateway/00-overview.md` and
`skills/04-review/ai-quota/regression-guards/anthropic-usage-observation-is-estimation-only.md`;
provider-header scope remains governed by
`skills/04-review/security/regression-guards/anthropic-translated-responses-rebuild-headers.md`.

## DSL Redis deletion command argument boundary

Extended from the request-stage DSL dispatch boundary at `d40513f36`. This is
ordinary DEL/HDEL preparation, separate from the previously inspected Lua script
executor and its key arrays. Paths below are relative to `edgion-gateway/src/`;
dependency observations use locked Fred 10.1.0.

| Boundary | Inspected work and retained owner |
| --- | --- |
| `plugins/http/dsl/lang/builtins.rs`, RedisDel / RedisHDel | Pop owned runtime strings, resolve the current Redis client through the existing alias map/store, release the alias and borrow key/field text into the command await. The client Arc and runtime strings remain caller-owned. Successful counts become `count > 0`; dependency errors retain the fixed public result and sensitive diagnostic contract. |
| `link_sys/providers/redis/ops.rs`, del / hdel | Construct Fred MultipleKeys directly from the borrowed slice, retaining order, duplicates and empty strings. The final Key payloads still own copied bytes. No intermediate Vec of borrowed references remains. The public slice signatures, typed pool commands, u64 result and error adapters are unchanged. |
| Fred `types/multiple.rs` and `types/args.rs` | FromIterator and the former Vec conversion both invoke Key::from(&str), which owns a copy of each byte sequence. Preparation happens in the provider async body's poll; no await separates it from polling Fred. HDEL now prepares fields before Fred copies the hash key, so allocation order is not claimed identical. |
| Fred `commands/interfaces/{keys,hashes}.rs` and `commands/impls/{keys,hashes}.rs` | DEL retains its empty-key check and ordered Value argument construction. HDEL retains hash-key-first command arguments and its distinct empty-field handling. Both retain typed command kinds, request_response dispatch and frame-to-result conversion. No custom_raw substitution, new batching or retry behavior is introduced. |

This boundary does not prove full Redis routing, wire encoding, reconnect or
cancellation correctness. Pool-member selection remains the separate redis-03
decision; live Redis and transport internals are outside this batch. Existing
[DSL error ownership](../../skills/04-review/dsl/regression-guards/redis-error-results-bounded-and-redacted.md)
and [Redis configuration](../../skills/04-review/link-sys/regression-guards/redis-configuration-contract.md)
remain authoritative. No cache, observability or credential policy is added.

## Ordinary Redis command ownership and current consumers

Inspected at `a7b8a76e4`, extending the preceding deletion boundary to other
ordinary DSL commands. This does not reopen the Lua executor, alias-release or
bounded-result decisions. Gateway owners are `plugins/http/dsl/lang/builtins.rs`
and `link_sys/providers/redis/ops.rs`; dependency sources are Fred 10.1.0's typed
interfaces, command implementations and Key/Value/Map conversions.

| Consumer group | Direct ownership and behavior |
| --- | --- |
| SET / SET_PX / HSET | RuntimeString produces owned arguments; the provider borrows them while Fred owns command Key/Value data. HSET's tuple becomes a one-entry Map before argument construction. SET client resolution precedes TTL-type rejection; positive integers use PX, while absent/nil/nonpositive integers omit expiration. HSET returns DSL true on any successful count. Input-interface choices extend existing dsl-01 rather than creating another task. |
| EXISTS / TTL / EXPIRE / INCR / INCRBY / HEXISTS | Retain typed provider/Fred operations and scalar results. EXISTS maps count > 0; TTL preserves negative sentinel integers; EXPIRE/INCRBY reject noninteger inputs before resolving the client. Keys and fields cross the await through existing owned caller strings and Fred's command data. No eager client capture or numeric formatting rewrite is selected. |
| GET / HGET | Fred returns Option<String>; absent values become nil, and successful text passes the existing UTF-8-boundary truncation before DSL Arc publication. These limits apply after dependency response aggregation. No claim of a network-frame memory bound follows. |
| Bounded GET / HGETALL | Existing raw-frame admission remains unchanged. GET validates bounded text before DSL publication; redis-06 owns duplicate validation. HGETALL counts original entries/bytes before owned-map conversion; redis-07 owns conversion/order tradeoffs. Do not replace either with the unbounded convenience method. |
| Management helpers | Current Gateway call sites in api/mod.rs use RPUSH/LPOP/LLEN, unbounded HGETALL, distributed lock and active health status. Redis try_lock has no production caller identified in this inspection; the similarly named API call is Etcd. These observations do not make public methods globally unreachable or prove a callback performance benefit from changing them. The periodic Redis health probe observes connection states, separately from the active PING helper. |

The [DSL consumer task](dsl-01-string-consumer-ownership.md#redis-write-command-consumer-follow-up)
records write-input ownership options while keeping values-only transfer separable
from key/field migration. Scalar coercion and fixed public error contracts remain
unchanged. This inspection does not authorize generic Redis values, new provider
APIs, retry changes, extra logs or a new cache. Ordinary command dispatch reaches
the existing request_response path; router, wire codec, reconnect and live backend
behavior retain their separate open boundaries.

## Redis caller timeout and dependency command lifetime

Inspected at `6f743246b`: HTTP RateLimitRedis and Stream ConnectionRateLimit's
outer timeout reaches the shared executor, then Fred 10.1.0 request_response.
This extends the earlier direct timeout and encoding inventories into command
ownership; it does not establish complete reconnect or transport coverage.

The basic waiter submits an owned Command with a response oneshot and shared
timed_out flag. Its own polled error path sets that flag; the tracing branch
has the corresponding post-result update. External future cancellation does
not execute these continuations. Neither inspected waiter has a drop guard
that propagates cancellation. Fred's timeout helper itself uses Tokio timeout.

The router checks timed_out before preparing/writing a command, and the retry
buffer checks it before resubmission. The connection retains response-bearing
commands before awaiting a transport write. Cancellation after the checks,
partial transmission and response ordering are distinct states. One configured
command attempt does not prove command cancellation or rollback.

The [cancellation ownership task](redis-09-cancelled-command-ownership.md) records
this behavior-sensitive opportunity separately from encoding and member selection.
No actual retained lifetime, remote outcome or memory/performance regression was
measured. The fresh 117 passing tests and one existing ignore cover current local
rate-limit/provider behavior, not live cancellation or reconnect interleavings.


## L4 listener and upstream transport-buffer boundaries

Revalidated at `7151f9d3f`, extending the earlier selection/relay inventory into
pinned Pingora `1aaa45cc70430bc782990bc20ddee3fef36d6b99` and Tokio 1.53.1.

| Boundary | Current source evidence | Limit |
| --- | --- | --- |
| TCP and TLS passthrough listener construction | Gateway `tcp_listeners` uses Pingora add_tcp variants, which preserve ListenerConfig's default L4 buffer settings. | No Gateway read-buffer override; this candidate does not include HTTP/HTTPS or retained termination code. |
| Accepted stream initialization | The raw constructor starts at zero capacities; listener handshake installs 64 KiB read / 1460-byte write buffers before PROXY protocol and optional TLS. | User-space reserved capacity, not resident or complete connection memory. |
| Relay reads | Each direction supplies an 8192-byte buffer. Tokio bypasses read buffering only for an empty internal buffer and a caller capacity at least that buffer's size. | Listener reads normally copy through the larger read buffer; read-ahead can reduce underlying calls. |
| Upstream transport | Built-in TCP connector retains zero-capacity stream buffers; TCP_NODELAY is an independent socket setting. | No missing-flush conclusion for this path; custom transports remain uninspected. |
| Downstream writes | Relay retains partial-write cursors and flushes each completed downstream write before the next read in that direction. | Read-buffer tuning does not authorize changing write buffering, flush ordering or fairness. |

[relay-02](relay-02-listener-read-buffer-sizing.md) defers workload-dependent
read-buffer sizing. Midstream buffer replacement is excluded because extracting
BufStream's inner stream can discard buffered bytes. Fresh unchanged-code tests
passed 135 shared-relay/TCP/TLS cases; formatting and workspace check/Clippy passed.
No live transport, benchmark, candidate behavior or complete dependency coverage
is established by those checks.


### PROXY setup read granularity and bounded probe replay

At `5a8be15ae`, the same pinned fork's optional trusted probe reads at most 12
bytes and rewinds the bytes read on success or returned EOF/I/O error. Disabled
and untrusted paths bypass probing; required mode parses directly. Returned
short EOF becomes no-header with preserved bytes, not a cancellation guarantee.
The parser consumes a 12-byte prefix, then v1 reads single bytes until LF and
validates CRLF within its 107-byte total cap (at most 95 additional byte reads).
Zero-capacity read buffering can therefore trade away setup read batching even
though relay reads are 8 KiB. Poll counts, application reads and kernel syscalls
must be measured separately.

Generic partial rewind reads copy the retained suffix, but the PROXY parser
consumes this probe's prefix whole. No large-payload copy claim or dependency
representation change follows. This extends [relay-02](relay-02-listener-read-buffer-sizing.md#setup-readers-constrain-the-sizing-choice)
with setup-specific acceptance cases rather than creating a second finding.
Fresh 48 Gateway PROXY-policy/helper and relay tests passed; they do not execute
the fork's parser/rewind tests or verify candidate transport behavior.


## Initial HTTP protocol detection before request callbacks

At `f6fcde124`, normal plaintext HTTP with HTTP/2 enabled reaches the pinned
fork's `HttpServerApp` application-entry h2c probe. TLS streams suppress that
probe and use ALPN; custom mode and unsupported Peek retain separate branches.
The concrete L4 probe reads and rewinds a complete 24-byte preface before byte
comparison, or restores the consumed prefix on returned EOF/I/O error. The
application returns None for such errors. Early mismatch does not short-circuit
the read loop. Ordinary HTTP/1 keepalive runs inside the selected H1 loop and
does not repeat this setup probe.

The listener's 60-second io.handshake timeout ends before application dispatch;
HTTP/1 keepalive and H2 session policies also do not own this pre-selection wait.
The short-H1 pending example and cancellation limits are recorded as source
inference in [transport-02](transport-02-h2c-preface-probing.md). Prefix-aware
fallback is deferred because earlier H1 admission changes timing/error behavior,
not just copying. No fork mutation, full-path timeout guarantee or measured
performance gain follows. Eight current Gateway H2/listener-builder tests passed;
these are not execution proof for a changed probe or live byte delivery.


## HTTP/1 connection-context and persistent-setting transfer

Revalidated at `477b5fe30` against pinned Pingora `1aaa45c`. This follows the two
previously inventoried Gateway connection callbacks into framework ownership,
not a change to the [total-request counting policy](../../skills/04-review/http1/regression-guards/downstream-keepalive-request-limit-counting.md).

| Boundary | Current ownership and ordering |
| --- | --- |
| Gateway persistence | `persist_connection_context` returns a boxed unit only when downstream_keepalive_initialized is true. The unit is zero-sized and requires no payload heap allocation; it is not a copied request context. |
| Ordinary finish | After logging and any proxy-failure notification, an allowed reuse snapshots scalar settings and takes the optional marker. Downstream finish must still return a reusable stream before those owners transfer. |
| Request-filter short circuit | A completed local response logs and cleans up subrequests before snapshotting settings and finishing the downstream session. It uses the same settings/stream ownership wrapper. |
| Reusable error completion | The error path reapplies forced-close state after mutable logging and requires both the response's reuse decision and ApplicationControlled reuse before capturing settings. The marker does not override closure. |
| Framework reuse | ReusedHttpStream consumes the reusable transport/prefix and settings by value. The H1 application loop constructs the next session, applies settings, saturating-decrements an optional remaining-reuse count, and moves user context and optional BytesMut prefix into it. It does not re-enter h2c selection. |
| Next request | HttpProxy takes connection user context before boxing the session and reading the next request. It creates a fresh Gateway context after successful request construction. Unsupported H1 Transfer-Encoding rejects before on_connection_reuse; otherwise the marker sets the new context's initialization flag before request processing. |
| Gateway early policy | Read/write/keepalive timeouts apply from the request snapshot, preserving transport-disabled reuse and process shutdown. The total-request limit is converted with checked_sub(1) only for the first H1 request, so later EGC updates cannot refill an established connection budget. |

The pinned H1 session defaults pipelining to false; no Gateway production caller
of set_pipelining_enabled was found. The generic framework nevertheless moves
optional prefix bytes rather than cloning them. This does not rule out injected
sessions with pipelining state, prove parser-prefix delivery, or authorize changing
that setting. The unit marker is semantic state; removing it would allow policy
reinitialization on reuse. Other session/context construction and async dispatch
allocations remain outside this narrow negative-allocation observation.

No behavior-preserving optimization was selected, and no duplicate deferred task
was created. Fresh Gateway early-request-policy and timeout tests passed 21 cases,
including the existing keepalive policy/wire tests. Formatting and workspace
all-target check/Clippy passed with existing warnings. These tests do not drive
every fork persistence branch, prove all error/cancellation interleavings, or
establish whole-request allocation/throughput properties.


## Generic KeyGet direct consumer reconciliation

Revalidated at `ec03fcb1a`. This is the current production caller inventory for
`PluginSession::key_get` and `session_adapter::key_value`, not an inference from
RequestId's admitted sources. Ordinary collection `get_key_value` calls are not
KeyGet consumers. Historical open-consumer notes in INDEX predate this inventory.

The canonical owned adapter covers twelve variants. The borrowed accessor handles
eleven directly and delegates only RemoteIpAndPath to the owned getter. Header,
Cookie, Ctx, current Path, Method and IP reads retain their execution-time
semantics; RequestPath reads the frozen entry path, TraceId/Host use captured
identity and Query borrows the lazy frozen entry-query cache. Lazy initialization may still allocate. No predicate result
or new source snapshot is cached. RemoteIpAndPath constructs one owned composite
when the resolved IP is nonempty; KeyExist checks just that presence directly.

| Direct consumer | Current ownership and policy boundary |
| --- | --- |
| DynamicInternalUpstream | Owned getter input supports regex capture, direct/configured target resolution and later jump publication. Input restructuring remains [routing-01](routing-01-dynamic-internal-input-ownership.md), distinct from the prepared rule index. |
| RESTToGraphQL | Moves an owned getter String into an owned JSON Value; body-derived values borrow the parsed body. Header-source multiplicity/text/length checks precede extraction. Existing binding transfer and unchanged-value bounds reuse remain in place. |
| Shared rate-limit resolution | Zero keys yield the global empty bucket; one key moves the owned getter result; multiple keys collect ordered borrowed/owned values and copy into one final framed String. Local limit, Redis named quotas and AI quota use this helper. Missing and present-empty semantics remain distinct. |
| RequestId adapter | Header uses strict single_header directly. The generic non-header branch calls key_get; ordinary runtime rules admit TraceId and Ctx, while early attachment permits only TraceId among non-header sources. None has no source read. See the separate admitted-source inventory above. |
| key_value fallback | Only RemoteIpAndPath currently reaches key_get. Adding a KeyGet variant requires rechecking this dispatch and all caller eligibility rules. |
| DynamicExternalUpstream | Borrowed/owned source supports extraction and configured domain lookup; callers retain fallback/reject policies and publish owned jump state. |
| DirectEndpoint | Borrowed/owned source feeds endpoint parsing and admission before owned target publication. Source reads are not cached across execution. |
| CtxSet | Value/template/from precedence is unchanged; from uses key_value with a borrowed default. Transformation and final context publication retain their own owned output. |
| KeyAuth | Only Header/Query/Cookie/Ctx are eligible. First nonempty selection remains independent of validation; an invalid selected credential cannot fall through to a later source. The read borrow ends before mutation/hiding or failure awaits. |
| RegionRoute | Its separate first-nonempty loop accepts configured source variants and hashes the selected value. It shares reads, not KeyAuth's eligibility policy. |
| Condition evaluator | KeyMatch, SecretMatch, keyed Probability and ordinary KeyExist use key_value synchronously. RemoteIpAndPath existence avoids composite construction; matching/hashing still require its value. Per-chunk evaluation retains live reads and emits no per-chunk PluginLog. |

The shared reads do not make all policies interchangeable. Original references
are the response-condition live-input borrowing guard, KeyAuth's separate
source-selection policy, the rate-limit shared extraction/composition guards and
the plugin development guide's entry/current identity contract. Existing routing
ownership decisions remain deferred. No new optimization or deferred task is
selected from these already covered caller boundaries.

Fresh unchanged-code verification passed a 420-test union selection over the
session adapter, condition evaluator, common helpers and the listed routing,
context, authentication and REST consumers. Formatting and workspace all-target
check/Clippy passed with existing warnings. This run does not execute every
rate-limit or AI quota consumer, establish dependency-internal coverage, measure
performance, or prove overall campaign completion. No new tests or fixtures changed.
