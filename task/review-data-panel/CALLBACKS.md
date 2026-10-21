# HTTP callback dispatch inventory

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

## Concrete remaining checks

- Fork successful Hold release currently transfers its suffix through a temporary
  Vec. The dependency-owned allocation candidate and required ownership tests are
  deferred in [response-02](response-02-fork-held-task-transfer.md); no fork change
  or complete barrier-lifecycle verification is claimed.

- Response-head ownership: direct owner orchestration, successful Hold retry
  closure and explicit pump-return cancellation are inspected above. Preselection
  failures, full callback multiplicity, deeper response execution, enclosing-future
  drops and transport/cancellation interleavings remain separate work.
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
