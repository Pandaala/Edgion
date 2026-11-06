# gw-plug3-01: Managed outbound health is one latching circuit for a whole endpoint pool; retries reuse the failed endpoint

- **Category**: design-error
- **Severity**: P1 (significant)
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/plugins/http/common/outbound/client.rs:60-80` (`HealthState`: one `healthy` bit per client)
  - `edgion-gateway/src/plugins/http/common/outbound/client.rs:769-855` (`run_active_health_check` probes one round-robin endpoint per tick; `record_active_health_result`)
  - `edgion-gateway/src/plugins/http/common/outbound/call.rs:297` (health gate), `:311` (`dial_url()` picked once), `:384-545` (retry loop reuses the same built request/URL)
  - `edgion-gateway/src/plugins/http/common/outbound/call.rs:672-735` (`record_passive_result`, `should_halfopen_probe`)
  - `edgion-gateway/src/plugins/http/common/outbound/pinned_resolver.rs:75-90` (Service pool rotation)
  - `edgion-resources/src/resources/common/external_http.rs:548-568` (`PassiveHealthCheck.count_timeout`, optional `backoff`)

## Summary
`ManagedOutboundClient` (ForwardAuth, route auth, Guardrail/AiGuard decision services, Webhook LinkSys)
keeps one health bit for the whole target. The target is often a `Service` / `ServiceEndpointSlice`
pool with many pods. A single bad pod can open the circuit for all pods, while a mostly-bad pool can
look healthy. Passive monitoring without `backoff` and without an active probe never recovers.
`countTimeout` is parsed and documented but never read. Retries go to the same endpoint that just
failed. For the fail-closed auth consumers, these defects become route-wide 5xx outages.

## Current behavior (evidence)
1. **One circuit per pool.** `HealthState { healthy: AtomicBool, passive_failures, .. }`
   (`client.rs:60-80`) is created once per client. `outbound_call` rejects every call with
   `Err(Unhealthy)` when `healthy == false` (`call.rs:297`). This happens regardless of which endpoint
   `pick_url()` would choose. Endpoints come fresh from `EpSliceStore` on every call (`target.rs:52-65`,
   `pinned_resolver.rs:75-90`). Health, however, is never kept per endpoint.
2. **The active probe samples one endpoint per tick.** `run_active_health_check` calls
   `target.pick_url()` once per interval (`client.rs:793`). With N endpoints and one dead pod:
   - If `unhealthyThreshold == 1` (or when bad probes happen to line up), one dead pod marks the client
     unhealthy. Every call to the N-1 healthy pods then fails until `healthyThreshold` consecutive good
     probes land.
   - If the threshold is greater than 1, the healthy and dead probes alternate. The consecutive
     counter resets on each healthy probe, so the dead pod is never removed, and 1/N of real calls keep
     hitting it.
3. **Passive-only without backoff latches forever.** `record_passive_result` sets `healthy = false`
   after `unhealthyThreshold` failures (`call.rs:684-692`). Recovery then needs either an active task
   or `passive.backoff`: `should_halfopen_probe` returns `false` when `health_task.is_some()` or when
   `backoff` is `None` (`call.rs:706-714`). `backoff` is `Option` with `#[serde(default)]`, and
   `validate_connection_target` does not require it (`external_http.rs:298-320`). So a config with only
   `healthCheck.passive: {unhealthyThreshold: 3}` turns three 503s into a permanent `Unhealthy`. Only a
   resource edit clears it, because that rebuilds the client (see gw-plugrt-02). For ForwardAuth
   (auth: fixed fail-close), this is a permanent 5xx for the route.
4. **`countTimeout` is dead configuration.** `rg count_timeout` finds only the field declaration.
   `call.rs:410,425` always record a passive failure on timeout and connect errors. The user-facing
   example in `skills/02-features/03-resources/18-link-sys.md` advertises `countTimeout: true`.
5. **Retries pin the failed endpoint.** The base URL is chosen once before the loop
   (`call.rs:311`). Every attempt is a `try_clone()` of the same `reqwest::Request`
   (`request_for_attempt`). For an IP-literal Service endpoint, `retryOnConnectError: true` therefore
   retries the same dead pod `maxRetries` times. A rewritten-authority request has the same problem:
   reqwest's keep-alive pool is per host, and `AnchorPinnedResolver` only rotates new connections.

## Industry comparison
- **Envoy**: active and passive health (outlier detection) are **per host** in a cluster. Ejection
  removes one host, and `max_ejection_percent` (default 10%) keeps one bad pod from ejecting the pool.
  Ejected hosts come back automatically after `base_ejection_time × n`. The ext_authz cluster uses
  exactly this machinery. Retries use `retry_host_predicate: previous_hosts` to choose another host.
- **Kong / APISIX upstream healthchecks**: per-target counters, with automatic re-enable through
  active probes or passive timeouts. A target, not the whole upstream, is marked down.
- **NGINX**: `max_fails` / `fail_timeout` per server. A failed server is retried after
  `fail_timeout` without any explicit backoff setting, and `proxy_next_upstream` picks another server.

## Impact
- A single crash-looping pod behind an auth Service can make the gateway reject all authenticated
  traffic, even though the remaining pods are healthy.
- A passive-only configuration, which is the simplest one an operator writes, causes a permanent
  outage after a short upstream blip. Nothing in the data plane can recover it.
- Retries add latency and load to the endpoint that just failed and seldom improve success.
- The documented `countTimeout: false` cannot stop timeouts from tripping the circuit.

## Suggested direction
1. Key health per endpoint for Service-pool targets: `HashMap<SocketAddr, HostHealth>` built from the
   current EndpointSlice and pruned on churn. `pick_url()` skips ejected endpoints, and an Envoy-style
   `maxEjectionPercent` guard keeps the pool usable. Keep the single circuit only for `Static`
   targets (URL / ClusterIP), where there is just one destination.
   - Alternative: reuse the backend outlier-detection component that already exists for main-proxy
     backends (`backends/`) instead of a second health model.
2. Make passive ejection always time-bounded. When `backoff` is omitted, use a default half-open
   interval (for example initial 5 s, max 60 s), or reject passive-only configurations without
   `backoff` at admission. Never latch forever.
3. Implement `countTimeout` (skip `record_passive_result(false)` for timeouts when it is `false`), or
   delete the field and the doc example (pre-release, no compatibility burden).
4. Re-run `dial_url()` and rebuild the request host for each retry attempt, excluding the previously
   tried endpoint when the pool has more than one.

## Notes
- Checked `skills/04-review/concurrency/regression-guards/halfopen-probe-toctou-cas.md`. It only
  covers CAS arbitration of the half-open window. It explicitly requires passive backoff to be
  configured and does not discuss the missing-backoff latch or per-endpoint scope.
- Checked `skills/04-review/concurrency/intentional-design/webhook-active-health-abort-only.md`
  (task lifecycle only).
- `gw-plugrt-02` covers `HealthState` being reset by unrelated edits, which is the opposite symptom.
  This finding is about the semantics of the state itself, so it does not duplicate gw-plugrt-02.
- `gw-infra-04` concerns main-proxy outlier ejection, not this plugin outbound engine.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P1 unchanged.
- `outbound/client.rs:60-80`: `HealthState` is one `healthy` bit per client; `call.rs:297` gates every call on it before `dial_url()` (`call.rs:311`) picks an endpoint, so health is never per endpoint.
- Passive latch re-checked: `record_passive_result` (`call.rs:672-695`) sets `healthy=false` and only seeds a recovery baseline when `backoff` is `Some`; `should_halfopen_probe` (`call.rs:704-714`) returns `false` when `backoff` is `None`. `PassiveBackoff` is `#[serde(default)] Option` (`external_http.rs:565-567`) and `validate_connection_target` (`external_http.rs:281-330`) never requires it. Passive-only without backoff therefore latches until a resource rebuild. The review page `halfopen-probe-toctou-cas.md` covers only the CAS window, not this latch.
- `count_timeout` is read nowhere in `edgion-gateway` (only the declaration `external_http.rs:562`, a resources unit test, and docs `connectors.md:994`, `18-link-sys.md:365`); timeouts always call `record_passive_result(false)` (`call.rs:410,425`).
- Caveat: the "one bad pod opens the circuit" path needs `unhealthyThreshold: 1` or a small pool, because round-robin interleaving resets the consecutive counter (`client.rs:832-850`). The latch and the dead knob carry the P1 on their own.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P1 unchanged)
- `plugins/http/common/outbound/` (`client.rs`, `call.rs`, `pinned_resolver.rs`) and `edgion-resources/src/resources/common/external_http.rs` have no diff; single `healthy` bit, passive latch without `backoff`, and unused `count_timeout` remain.
