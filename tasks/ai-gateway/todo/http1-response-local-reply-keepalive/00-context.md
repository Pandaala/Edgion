# Context: HTTP/1 Response Local-Reply Keepalive

Read the main task first:

- `tasks/todo/http1-response-local-reply-keepalive/http1-response-local-reply-keepalive.md`

## Required project knowledge

- `skills/00-design-rules.md` — especially HTTP/proxy correctness, hot-path
  performance, change discipline, and testing discipline.
- `skills/01-architecture/02-gateway/SKILL.md` — Gateway HTTP proxy architecture
  entry point.
- `skills/01-architecture/02-gateway/05-plugin-system.md` — response-stage plugin
  execution and local-reply behavior.
- `skills/03-coding/01-log-safety.md` — no tracing on the Gateway data-plane hot
  path.
- `skills/05-testing/01-integration-testing.md` — integration scenario and
  targeted-run conventions.
- `skills/07-tasks/new_feature_work_flow.skill` — brainstorming-to-delivery
  workflow.
- `skills/04-review/architecture/response-stage-denial-must-terminate.md` — why a
  refused or written response-stage denial must stop upstream forwarding.
- `skills/04-review/architecture/local-reply-drops-sticky-set-cookie.md` — sibling
  filter-04c decision that is explicitly out of scope.
- `skills/04-review/rfc-divergence/no-connection-close-injection.md` — Pingora's
  codec, rather than Edgion header injection, owns `Connection` wire framing.

## Edgion code entry points

- `edgion-gateway/src/routes/http/proxy_http/pg_response_filter.rs`
  - Writes the response-stage local reply.
  - Calls `session.set_keepalive(None)`.
  - Returns the sentinel error.
- `edgion-gateway/src/routes/http/proxy_http/pg_fail_to_proxy.rs`
  - Recognizes `ctx.local_reply_written`.
  - Suppresses a duplicate error response.
  - Returns `can_reuse_downstream: true`.
- `edgion-gateway/src/routes/http/proxy_http/pg_error_while_proxy.rs`
  - Keeps the sentinel non-retryable and avoids upstream failure accounting.
- `edgion-gateway/src/routes/http/proxy_http/mod.rs`
  - Suppresses the expected sentinel error log.
- `edgion-gateway/src/ctx.rs`
  - Owns `local_reply_header`, `local_reply_body`, and
    `local_reply_written` lifecycle state.
- `edgion-gateway/src/plugins/runtime/session_adapter.rs`
  - Builds and stashes response-stage local replies.

## Edgion test entry points

- `edgion-tests/src/client/suites/edgion_plugins/dsl/dsl.rs`
  - Existing `dsl_resp_deny_plain` and
    `dsl_resp_deny_survives_unrecognized_upgrade` tests.
- `edgion-tests/integration/conf/EdgionPlugins/Dsl/30_EdgionPlugins_dsl-resp-deny-upgrade.yaml`
  - Existing response-stage `deny(403, ...)` plugin.
- `edgion-tests/integration/conf/EdgionPlugins/Dsl/HTTPRoute_dsl-resp-deny-upgrade.yaml`
  - Route for the response-stage denial scenario.

## Pingora fork entry points

Local repository: `/Users/caohao/ws2/pingora`

- `pingora-proxy/src/lib.rs`
  - `HttpProxy::process_request`: post-pump final-error path drops the
    `can_reuse_downstream` result.
  - `HttpProxy::handle_error`: pre-pump path already honors it.
  - `HttpProxy::finish`: calls downstream `finish()` only when reuse is allowed.
- `pingora-proxy/src/proxy_trait.rs`
  - `ProxyHttp::fail_to_proxy` contract and `FailToProxy` definition.
- `pingora-core/src/protocols/http/server.rs`
  - Transport-neutral `finish()` and `response_body_finished()` accessors.
- `pingora-core/src/protocols/http/v1/server.rs`
  - Response framing, automatic close on early response, `will_keepalive()`,
    request-body drain, overread rejection, and `reuse()`.
- `pingora-proxy/tests/test_request_body_seam.rs`
  - Self-contained raw H1 helpers that consume exactly one response and probe
    connection reuse without requiring OpenResty.

## Baseline revisions observed during triage

- Edgion brainstorming checkout HEAD: `89f9e241773f40a468914a7a251867d9bfaf38ad`
- Pingora fork branch: `edgion`
- Pingora fork HEAD and Edgion locked revision:
  `b1d520ea24b4225ff3dcfa97b1493b919d363c13`

Re-check these revisions and both worktrees before implementation; they are
context, not a request to reset either repository.
