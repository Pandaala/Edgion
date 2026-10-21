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

## Concrete remaining checks

- Response-head ownership: pg_response_head prioritizes the transcoder plan,
  then unary conversion, then the active Guardrail claim. Inspect those owners'
  plan/boundary decisions and final eligibility against current callback ordering.
  Direct will-commit excludes 101, claims the final writer and seals cache headers;
  its tests do not prove every owner or transport interleaving.
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
