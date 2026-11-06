# gw-ext-02: ExtProc caps each target at 64 concurrent streams and uses one unbalanced HTTP/2 connection

- **Category**: design-error
- **Severity**: P1 (significant)
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/http/ext_proc/client.rs:41-53` (`TARGET_SESSION_CAP`, `PERMIT_WAIT_CAP`, `TARGET_RUNTIMES`), `client.rs:160-207` (`ExtProcClient::new`), `client.rs:209-250` (`build_channel`), `client.rs:259-276` (`open_stream`), `edgion-gateway/src/plugins/http/ext_proc/attached.rs:182-243`, `edgion-gateway/src/plugins/http/ext_proc/plugin.rs:154-186` (`fail_mode`)

## Summary
All ExtProc entries that resolve to the same target and TLS identity share one process-wide
`Semaphore(64)`. One permit is held for each request's `Process` stream for the whole
transaction. When response phases are enabled, that includes the upstream wait and the
entire streamed response. When no permit is free within `min(timeout, 100 ms)`, the request
fails with `resource_exhausted`. With the default `failureModeAllow: false` that is HTTP 500;
with fail-open the processor is silently skipped. All streams to the target also share a
single lazily connected tonic `Channel`, which is one HTTP/2 connection to one resolved
address. In practice, the gateway uses at most one processor replica behind a Kubernetes
Service.

## Current behavior (evidence)
- `const TARGET_SESSION_CAP: usize = 64; const PERMIT_WAIT_CAP: Duration = 100ms`
  (`client.rs:42-43`). These constants cannot be configured.
- `ExtProcClient::new` caches an `ExtProcTargetRuntime { client, sessions: Semaphore(64) }`
  keyed by target, effective TLS and the egress policy. Every plugin entry, route and
  EdgionPlugins resource that points at the same processor shares it (`client.rs:183-206`).
- `open_stream` waits at most `min(timeout, 100ms)` for a permit and otherwise returns
  `Status::resource_exhausted("ext_proc target session limit reached")` (`client.rs:259-265`).
  The permit is stored in `ExtProcStream._permit`.
- `AttachedExtProc` keeps the stream in `StreamState::Open` between phases
  (`attached.rs:232-239`). With `responseHeaderMode: SEND`, `responseBodyMode: STREAMED` or
  `responseTrailerMode: SEND`, the permit therefore lives until the response terminal.
- Every exchange failure is routed to `fail_mode`, which returns
  `ErrResponse { status: 500 }` or `GoodNext` (`plugin.rs:176-185`).
- `build_channel` uses `Channel::from_shared(...).connect_with_connector_lazy(connector)`
  (`client.rs:225-250`). A tonic `Channel` built from one endpoint is one connection with
  reconnect. `connect_admitted` returns the *first* address that connects
  (`client.rs:545-567`). All gateway→processor traffic goes over this one TCP connection. A
  ClusterIP Service pins it to one pod for the connection's lifetime, and headless or
  multi-A records are never balanced.

## Industry comparison
- **Envoy ext_proc**: `grpc_service` points at an Envoy *cluster*. The cluster has EDS/DNS
  endpoint discovery, a load-balancing policy, health checks, outlier detection and an
  HTTP/2 connection pool. Its concurrency limits are cluster circuit breakers
  (`max_requests`, default 1024; `max_pending_requests`), which are configurable and have
  their own `upstream_rq_pending_overflow` stats. Pool exhaustion leads to 503-class overflow,
  not to a fixed 64-stream ceiling per target.
- **Envoy `failure_mode_allow`** governs processor *failure*. Local overload is reported
  through circuit-breaker stats so operators can tell it apart from processor errors.
- **tonic** offers `Channel::balance_list` / `balance_channel` for client-side balancing
  across endpoints. Edgion already resolves endpoints through `EgressResolver`.

## Impact
- Any route that uses ExtProc with response processing (header enrichment, response
  scanning) is limited to 64 concurrent requests per gateway process *per processor target*,
  shared across every route that uses the processor. LLM/SSE streams make this ceiling easy
  to reach.
- Reaching the limit yields 500s (fail-close) or a silent policy bypass (fail-open). Neither
  outcome is distinguishable from a processor outage in the access log
  (`ext_proc ...:fail-close` with reason `resource_exhausted`).
- Scaling the processor Deployment does not help, because each gateway process talks to only
  one replica over one connection. Processor-side HTTP/2 `MAX_CONCURRENT_STREAMS` (default
  100 in many servers) also becomes a hidden second limit on that one connection.

## Suggested direction
1. Replace the fixed cap with a configurable per-target concurrency limit that has a large
   default (for example 1024, as with Envoy's circuit-breaker defaults), or remove the
   semaphore and rely on gateway-level overload protection. Map exhaustion to 503 with a
   distinct reason or metric label, separate from processor failures.
2. Build the channel over all resolved endpoints, re-resolved on the existing egress DNS
   TTL, using `tonic::transport::Channel::balance_channel`. Alternatively, keep a small pool
   of N connections and pick per stream. This gives multi-replica spreading and avoids
   head-of-line blocking from processor `MAX_CONCURRENT_STREAMS`.
3. Optionally release the permit early when no future response phase is enabled. The code
   already computes `has_future_response_phase`.

## Notes
- No review-knowledge page covers `TARGET_SESSION_CAP` or the single-channel design. Checked
  `skills/04-review/ext-proc/*` (four regression guards on headers, TLS, phase matching and
  continue-and-replace) and `skills/01-architecture/00-common/05-unification-points.md`
  (outbound TLS/egress resolver). Nothing records the cap as an accepted tradeoff.
- `docs/en/user-guide/http-route/filters/edgion-plugins/ext-proc.md` does not document the
  64-stream ceiling, so operators cannot plan around it.
- Related: `arch-02-no-process-level-overload-protection.md`. A gateway-level overload
  manager would be the right owner for global shedding instead of this per-target constant.

## Verification (independent)

- **Verdict**: CONFIRMED (severity P1 unchanged)
- `edgion-gateway/src/plugins/http/ext_proc/client.rs:42-43`: non-configurable `TARGET_SESSION_CAP = 64`, `PERMIT_WAIT_CAP = 100ms`; `open_stream` (`:259-276`) returns `resource_exhausted` on timeout and stores the permit in the stream.
- `attached.rs:230-237`: the stream (and permit) is kept in `StreamState::Open` while further phases remain, so response-phase configurations hold it through the response.
- `client.rs:229,246`: one lazy tonic `Channel` per target via `connect_with_connector_lazy`; no `balance_channel`/`balance_list` anywhere in `ext_proc/`.
- No `skills/04-review/ext-proc/*` page records the cap or single-channel design as accepted.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P1 unchanged)
- `ext_proc/client.rs`, `attached.rs` and `plugin.rs` are unchanged since feature-08-30; `TARGET_SESSION_CAP = 64` is still at `client.rs:42`.
- `05-unification-points.md` changed only for the Redis fred -> redis-rs migration; no ext_proc capacity rationale was added.
- No ext-proc review page addressing the session cap was added (`opaque-header-fidelity.md` changed by one line, unrelated).
