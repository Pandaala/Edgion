# gw-core-01: SIGTERM stops accepting immediately while `/ready` stays 200, and the drain outlives the default pod grace period

- **Category**: design-error
- **Severity**: P1 (significant)
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/api/mod.rs:85-98` (`readiness_check` / `readiness_status`)
  - `edgion-gateway/src/cli/pingora.rs:42-44` (grace 30s + graceful shutdown timeout 10s defaults)
  - `edgion-gateway/src/cli/mod.rs:276-300` (probe server only stops on the post-Pingora auxiliary shutdown watch)
  - Pinned Pingora `pingora-core/src/server/mod.rs` `main_loop` (`GracefulTerminate` → `shutdown_watch.send(true)` immediately) and `pingora-core/src/services/listening.rs` `run_endpoint` (accept loop `break`s on the shutdown watch)
  - `cicd/deploy/kubernetes/gateway/deployment.yaml` (no `preStop`, no `terminationGracePeriodSeconds`)

## Summary
On SIGTERM the pinned Pingora fork broadcasts the shutdown watch at once, and every TCP accept loop exits
before the 30-second grace sleep begins. Edgion's `/ready` keeps returning 200 throughout that window
because readiness only checks config-sync readiness and the PROXY-policy fatal flag, and the probe server is
stopped only after `Server::run` returns. The shipped Deployment has no `preStop` delay and inherits
Kubernetes' default 30-second `terminationGracePeriodSeconds`. That is shorter than Edgion's own drain
schedule: 30 seconds of grace, a 10-second graceful timeout, and 5 seconds of auxiliary drain. Every rolling
update or scale-in therefore has two avoidable failures. First, new connections are refused or blackholed
while endpoint removal propagates. Second, in-flight requests are SIGKILLed during the configured drain.

## Current behavior (evidence)
- Pingora `main_loop`: `ShutdownSignal::GracefulTerminate => { ... self.shutdown_watch.send(true) ... ShutdownType::Graceful }`.
  The grace sleep (`thread::sleep(Duration::from_secs(exit_timeout))`, `server/mod.rs:~800-812`) happens
  *after* the broadcast.
- Pingora `run_endpoint`: `shutdown_signal = shutdown.changed() => { ... if *shutdown.borrow() { break; } }`.
  The listener stops calling `accept()` as soon as SIGTERM arrives. During the grace period, new SYNs either
  sit in the kernel backlog until the client times out or are reset once the endpoint's stack is dropped.
- `api/mod.rs:85-98`: `readiness_status(client.is_ready().is_ok(), policy_fatal)` has no shutdown input,
  so the endpoint keeps reporting 200.
- `cli/mod.rs:276-300`: the probe server's shutdown future waits on `shutdown_rx`. The startup/shutdown
  canon (`skills/01-architecture/02-gateway/01-startup-shutdown.md` §Shutdown steps 3-4) says this watch fires
  only after `Server::run(RunArgs)` returns, which is after the grace and graceful-shutdown windows.
- `deployment.yaml` sets `maxUnavailable: 0` to protect rollouts but has no `lifecycle.preStop` and no
  `terminationGracePeriodSeconds`. The Kubernetes default is 30s, while Edgion's defaults need about 45s.
- In practice, external load balancers and Kubernetes `Endpoints`/`EndpointSlice` consumers outside the
  kubelet react to readiness or endpoint removal with a lag of seconds. For example, cloud NLB health checks
  poll /ready, and kube-proxy and ingress-of-ingress setups propagate through informers. Traffic still
  arrives during that lag, after Edgion has already stopped accepting.

## Industry comparison
- **Envoy / Istio**: SIGTERM, or a pilot-agent drain, puts Envoy into *drain* mode. `/ready` fails
  immediately through `healthcheck/fail`. Listeners **keep accepting** for `drainDuration` while HTTP/1
  responses carry `Connection: close` and HTTP/2 connections receive GOAWAY. Only then are listeners closed.
  Istio also sets `terminationDrainDuration` and documents the required `terminationGracePeriodSeconds`.
- **Envoy Gateway / Contour**: ship a shutdown-manager `preStop` hook that fails readiness, waits for active
  connections to drain, then lets Envoy exit, and set `terminationGracePeriodSeconds` to cover it.
- **ingress-nginx**: `preStop: /wait-shutdown` and `terminationGracePeriodSeconds: 300`. NGINX keeps
  accepting until the graceful quit.

## Impact
- Rolling upgrades of the Gateway, a routine operation, produce connection-refused or connect-timeout errors
  for clients whose LB or kube-proxy has not yet observed the pod's removal. `maxUnavailable: 0` cannot help
  because the old pod is already terminating.
- With default manifests, long requests and streams still in flight at second 30 are SIGKILLed during
  Edgion's own graceful-shutdown timeout. The configured `graceful_shutdown_timeout_seconds` therefore never
  fully runs in Kubernetes.
- `/ready` reports a pod that no longer accepts connections as ready, which misleads external health
  checkers.

## Suggested direction
1. Flip readiness on shutdown. Subscribe to Pingora's `execution_phase_watch` (it already emits
   `ExecutionPhase::GracefulTerminate`) or to an Edgion signal hook, and make `/ready` return 503 from the
   moment SIGTERM is received.
2. Keep accepting during a configurable *drain* window before stopping accept. This is Envoy's
   `drainDuration` / Istio's `terminationDrainDuration` model: keep listeners open, use the existing
   keepalive-disable plus `Connection: close` path, send H2 GOAWAY, and only then broadcast the Pingora
   shutdown watch. In the fork this means delaying `shutdown_watch.send(true)` by a drain duration on
   `GracefulTerminate`, or adding an accept-drain phase. Without a fork change, the minimum fix is a
   documented `preStop` sleep together with item 1.
3. Ship correct manifests. Add `terminationGracePeriodSeconds` ≥ drain + `grace_period_seconds` +
   `graceful_shutdown_timeout_seconds` + 5s auxiliary drain, plus a `preStop` hook, to
   `cicd/deploy/kubernetes/gateway/deployment.yaml`. Also add a startup WARN when the configured schedule
   cannot fit, if the pod grace period can be known through the downward API.

## Notes
- Checked `skills/04-review/concurrency/regression-guards/proxy-shutdown-notification.md`. It covers
  shutdown *notification* ordering inside the fork, not readiness or accept timing.
- Also checked `listeners/regression-guards/udp-process-shutdown.md` and
  `skills/01-architecture/02-gateway/01-startup-shutdown.md`. Neither addresses readiness during the grace
  period or the pod grace budget.
- No `tasks/` entry mentions `preStop`, `terminationGracePeriodSeconds` or shutdown readiness.

## Verification (independent)

- **Verdict**: CONFIRMED (severity P1 unchanged)
- Pinned fork `e5f4287` `pingora-core/src/server/mod.rs:252-268` sends `shutdown_watch.send(true)` on `GracefulTerminate` before the `thread::sleep(exit_timeout)` at `:805-811`; `services/listening.rs:211-214` breaks the accept loop as soon as the watch flips.
- `edgion-gateway/src/api/mod.rs:85-98`: `readiness_status(config_ready, policy_fatal)` has no shutdown input; no `execution_phase`/`ExecutionPhase` subscriber exists anywhere in `edgion-gateway/src`.
- `edgion-gateway/src/cli/pingora.rs:42-44`: defaults 30s grace + 10s graceful timeout; `cicd/deploy/kubernetes/gateway/deployment.yaml` has no `preStop` and no `terminationGracePeriodSeconds` (K8s default 30s).
- No `skills/04-review` page accepts this shutdown/readiness sequence.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P1 unchanged)
- `edgion-gateway/src/api/mod.rs:85-98`: `readiness_status(config_ready, policy_fatal)` unchanged; the only diff in this file is the Redis admin ping at `:929`.
- `edgion-gateway/src/cli/pingora.rs:42-44` (30s grace + 10s graceful timeout) and `cicd/deploy/kubernetes/gateway/deployment.yaml` (no `preStop`/`terminationGracePeriodSeconds`) have no diff.
- Pingora fork still resolves to `e5f4287be713d01ffd444544251bf75c9d00062a` in `Cargo.lock`, so the accept-loop/shutdown-watch ordering is unchanged.
