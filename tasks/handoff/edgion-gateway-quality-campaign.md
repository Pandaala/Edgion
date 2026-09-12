# Edgion Gateway Quality Campaign Handoff

> Resume with `$edgion-gateway-quality-campaign` in continue mode. Do not create another campaign, do not invoke `fix-issue-cn`, and never push.

## Current state

| Key | Value |
|-----|-------|
| Repository | `/Volumes/ExtStore/ws5/Edgion` |
| Branch | `feature-08-30` |
| Recovery record | This file |
| Active anchor | `edgion-gateway/` |
| Processed files | 445 |
| Ignored files | 28 |
| Remaining files | 78 |
| Active batch | None |
| Running analysis | None |
| Previous integration freeze | 3,274 existing `edgion-tests/**` paths; stale after attributed external commits |

Execution was intentionally paused by the user on 2026-09-11. No campaign-owned analysis, implementation, test, or Cargo process is running.

## Recovery contract

This handoff is the durable recovery record after the former coordinator
directory is removed. The processed count is historical summary only. The
ignored and remaining lists below are the complete live scope needed to resume.

Before delegating any new analysis, create a fresh campaign directory using the
current campaign skill, seed its ledger from these lists, reconcile every listed
path against the current tree, and create a new integration-freeze manifest only
after attributing current `edgion-tests/**` worktree changes. Do not treat the
previous 3,274-path snapshot as valid: it already detected externally committed
drift at pause time. Rebuilding this state after the former directory is removed
is restoration of this paused campaign, not authorization for a second campaign.

After a batch commits, move its target paths from remaining to processed and
update the counts. Do not add batch IDs, commit hashes, issue IDs, outcome notes,
or task links to the file lists.

Historical batch and changelog records are optional evidence available from Git
history. They do not own live status and are not required to complete or delete
a standalone follow-up task.

The former campaign-local deferred mirror and shared index were removed.
Twenty-six still-active mirror-only findings were migrated into self-contained
`tasks/todo/` files before removal; existing standalone tasks were not
duplicated. A task must not depend on this campaign, refer back to a batch or
deferred ID, or require any campaign update. Another agent may complete and
delete the task file directly.

## Fixed scope

The 78 paths under `Remaining files` are the complete future target set. The 28
ignored paths cover pending AI-owned targets and standalone
`edgion-gateway/tests/**` targets. Newly created or discovered Gateway files are
outside the campaign unless the user explicitly changes the remaining list.

Analysis may read surrounding code and knowledge. Optimization must start from a remaining target. Any supporting edit outside that list requires direct correctness evidence and an exact batch allow-list; it does not become another coverage target.

### Ignored files (28)

- `edgion-gateway/src/ai/adaptive.rs`
- `edgion-gateway/src/ai/anthropic/mod.rs`
- `edgion-gateway/src/ai/anthropic/request.rs`
- `edgion-gateway/src/ai/anthropic/response.rs`
- `edgion-gateway/src/ai/attempt.rs`
- `edgion-gateway/src/ai/attempt_plan.rs`
- `edgion-gateway/src/ai/attempt_runtime.rs`
- `edgion-gateway/src/ai/backend_store.rs`
- `edgion-gateway/src/ai/canonical_sse.rs`
- `edgion-gateway/src/ai/circuit_breaker.rs`
- `edgion-gateway/src/ai/credential_outlier.rs`
- `edgion-gateway/src/ai/dispatch.rs`
- `edgion-gateway/src/ai/error.rs`
- `edgion-gateway/src/ai/json_span.rs`
- `edgion-gateway/src/ai/mod.rs`
- `edgion-gateway/src/ai/model_router.rs`
- `edgion-gateway/src/ai/quota/mod.rs`
- `edgion-gateway/src/ai/quota/runtime.rs`
- `edgion-gateway/src/ai/routing.rs`
- `edgion-gateway/src/ai/terminal.rs`
- `edgion-gateway/src/ai/usage.rs`
- `edgion-gateway/src/plugins/http/ai_guard/evaluator.rs`
- `edgion-gateway/src/plugins/http/ai_proxy/plugin.rs`
- `edgion-gateway/src/plugins/http/guardrail/attached.rs`
- `edgion-gateway/src/plugins/http/guardrail/response.rs`
- `edgion-gateway/tests/conf_sync_failover.rs`
- `edgion-gateway/tests/geoip_fixture_readback.rs`
- `edgion-gateway/tests/tls_passthrough_clienthello.rs`

### Remaining files (78)

- `edgion-gateway/Cargo.toml`
- `edgion-gateway/benches/lru_expiration.rs`
- `edgion-gateway/build.rs`
- `edgion-gateway/src/api/admin_auth.rs`
- `edgion-gateway/src/api/mod.rs`
- `edgion-gateway/src/backends/discovery/coordinator.rs`
- `edgion-gateway/src/backends/degrade_threshold.rs`
- `edgion-gateway/src/backends/policy/backend_traffic/store.rs`
- `edgion-gateway/src/backends/selector.rs`
- `edgion-gateway/src/backends/tests/backend_tls_pool_test.rs`
- `edgion-gateway/src/cli/config.rs`
- `edgion-gateway/src/cli/mod.rs`
- `edgion-gateway/src/cli/pingora.rs`
- `edgion-gateway/src/config/edgion_gateway/conf_handler_impl.rs`
- `edgion-gateway/src/ctx.rs`
- `edgion-gateway/src/lb/attempt.rs`
- `edgion-gateway/src/lb/backend_selector/mod.rs`
- `edgion-gateway/src/lb/backend_selector/selector.rs`
- `edgion-gateway/src/lb/backend_selector/weighted_selector.rs`
- `edgion-gateway/src/lb/compiled_selector.rs`
- `edgion-gateway/src/lb/backend.rs`
- `edgion-gateway/src/lb/ewma/score.rs`
- `edgion-gateway/src/lb/ewma/mod.rs`
- `edgion-gateway/src/lb/leastconn/cleaner.rs`
- `edgion-gateway/src/lb/leastconn/mod.rs`
- `edgion-gateway/src/lb/mod.rs`
- `edgion-gateway/src/lb/runtime_state/mod.rs`
- `edgion-gateway/src/lb/sample.rs`
- `edgion-gateway/src/lb/selection/consistent_hash.rs`
- `edgion-gateway/src/lb/selection/mod.rs`
- `edgion-gateway/src/lb/selection/p2c.rs`
- `edgion-gateway/src/lb/selection/round_robin.rs`
- `edgion-gateway/src/lib.rs`
- `edgion-gateway/src/link_sys/providers/otlp/log_sender.rs`
- `edgion-gateway/src/link_sys/providers/webhook/manager.rs`
- `edgion-gateway/src/link_sys/runtime/data_sender.rs`
- `edgion-gateway/src/link_sys/runtime/store.rs`
- `edgion-gateway/src/observe/metrics/registry.rs`
- `edgion-gateway/src/plugins/http/common/outbound/call.rs`
- `edgion-gateway/src/plugins/http/common/outbound/request.rs`
- `edgion-gateway/src/plugins/http/mod.rs`
- `edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs`
- `edgion-gateway/src/plugins/runtime/plugin_runtime.rs`
- `edgion-gateway/src/plugins/runtime/session_adapter.rs`
- `edgion-gateway/src/plugins/runtime/traits/session.rs`
- `edgion-gateway/src/plugins/stream/connection_filter_bridge.rs`
- `edgion-gateway/src/plugins/stream/connection_rate_limit/local_gate.rs`
- `edgion-gateway/src/plugins/stream/connection_rate_limit/plugin.rs`
- `edgion-gateway/src/plugins/stream/geoip_location/plugin.rs`
- `edgion-gateway/src/plugins/stream/mod.rs`
- `edgion-gateway/src/plugins/stream/stream_plugin_runtime.rs`
- `edgion-gateway/src/plugins/stream/stream_plugin_store.rs`
- `edgion-gateway/src/plugins/stream/stream_plugin_trait.rs`
- `edgion-gateway/src/plugins/stream/tls_route/mod.rs`
- `edgion-gateway/src/routes/http/conf_handler_impl.rs`
- `edgion-gateway/src/routes/http/proxy_http/mod.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_early_request_filter.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_fail_to_connect.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_fail_to_proxy.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_logging.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_request_body_filter.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_response_filter.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_response_head.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_request_filter.rs`
- `edgion-gateway/src/routes/http/proxy_http/preflight_handler.rs`
- `edgion-gateway/src/routes/http/redirect_http.rs`
- `edgion-gateway/src/routes/tls/mod.rs`
- `edgion-gateway/src/routes/tls/passthrough_proxy.rs`
- `edgion-gateway/src/routes/tls/proxy.rs`
- `edgion-gateway/src/routes/tls/routes_mgr.rs`
- `edgion-gateway/src/routes/tls/tests.rs`
- `edgion-gateway/src/routes/udp/routes_mgr.rs`
- `edgion-gateway/src/routes/udp/udp_route_table.rs`
- `edgion-gateway/src/runtime/matching/tls.rs`
- `edgion-gateway/src/runtime/server/error_response.rs`
- `edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs`

## Resume point

Start a fresh read-only analysis from these four remaining plugin-runtime targets, then let the analyst decide whether they form one cohesive batch or should be narrowed:

- `edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs`
- `edgion-gateway/src/plugins/runtime/plugin_runtime.rs`
- `edgion-gateway/src/plugins/runtime/session_adapter.rs`
- `edgion-gateway/src/plugins/runtime/traits/session.rs`

They were affected or reopened by RequestId/request-trace-id migration
`79b72bb3`. A provisional analysis was interrupted before producing a report;
do not infer any conclusion from it or reuse it.

## Required controls

- Read the repository rules and campaign skill before resuming.
- Verify the processed summary count and reconcile the complete ignored and
  remaining lists against the current tree.
- Establish the new frozen integration manifest exactly as described in the
  recovery contract before delegating a batch.
- Keep technical analysis, implementation, and independent review in separated agent roles.
- Keep exactly one mutation/review/verification/commit lane.
- Preserve unrelated concurrent work; HEAD movement alone is not a blocker.
- Never broadly stage, reset user work, edit existing `edgion-tests/**`, or push.
- Run `cargo test --all` only when explicitly requested.

The campaign simplification is intentionally record-oriented: this handoff owns
the recovery scope, historical records are immutable, and standalone tasks have
no shared lifecycle state.

At handoff, the frozen manifest check detects externally committed changes under `edgion-tests/**`. The simplification did not modify those files. Reconcile only verified external paths on resume; never reset or rewrite them to satisfy the manifest.
