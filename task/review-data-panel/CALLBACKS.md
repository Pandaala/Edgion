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
| `response_body_filter` | Ok(None), no downstream body change/delay |
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
  ResponseCache internals remain a separate inspection scope.
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
16, reconciling 24 of 60 explicit RequestFilter implementations. The remaining
36 entries still need inventory reconciliation. Paths below are relative to
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
