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

Search of Gateway sources found no session.cache enable call. This supports a
narrow source observation, not a complete reachability proof. Edgion's own
ResponseCache lifecycle is distinct from these fork cache callbacks. Re-evaluate
on fork upgrades, new trait overrides, feature changes or cache configuration.

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
- Fork cache defaults: verify actual enablement and guarded call sites before
  classifying key/range/purge/stale work as reachable or excluded. Keep Edgion
  ResponseCache inspection separate.
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
