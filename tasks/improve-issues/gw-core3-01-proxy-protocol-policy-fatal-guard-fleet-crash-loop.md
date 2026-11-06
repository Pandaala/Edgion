# gw-core3-01: One Gateway's inbound PROXY state (invalid, conflicting, or edited) kills every Gateway replica and crash-loops startup

- **Category**: design-error
- **Severity**: P0 (availability, fix now)
- **Scope**: large-direction
- **Location**:
  - `edgion-gateway/src/runtime/server/proxy_protocol_policy.rs:171-196` (`observe_snapshot`), `:199-233` (`preflight_and_freeze`), `:236-268` (`trip`), `:355-447` (`aggregate_snapshot` / `add_gateway_contributions`: `Invalid` at `:395-397`, `SharedListenerPolicyConflict` at `:418-430`), `:81-85` (`equivalent_socket_semantics`), `:549-558` (`GuardFirstConnectionFilter::should_accept`)
  - `edgion-gateway/src/runtime/handler.rs:94-99` (`full_set`), `:239-245` (`partial_update`): every in-scope Gateway is aggregated
  - `edgion-gateway/src/runtime/server/base.rs:82-85` (startup `preflight_and_freeze(...)?`)
  - `edgion-gateway/src/cli/pingora.rs:161-170` (fatal → `ShutdownSignal::FastShutdown`), `edgion-gateway/src/api/mod.rs:85-98` (`/ready` → 503)
  - Controller: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/gateway.rs:671-690` (writes `ResolvedInboundProxyProtocol::Invalid`, including `SharedListenerConflict` for **every** contributor of a conflicting physical listener), `.../proxy_protocol_listener_policy.rs:145-147`

## Summary
The data plane treats the inbound PROXY-protocol policy as frozen socket semantics. After startup, any
Gateway snapshot whose aggregate differs from the frozen baseline, or that contains any Gateway in the
`Invalid` state, flips a process-global fatal latch: all TCP accept filters start rejecting, `/ready`
returns 503 and the process FastShuts down. The same aggregate also gates startup, so the restarted
process fails `configure_listeners` and crash-loops. Because every replica receives the same snapshot at
the same moment, a single Gateway object (a typo in one annotation, a missing `ProxyProtocolTrust`
reference, a second Gateway on a shared port, or a routine trusted-CIDR change) takes the whole data
plane down simultaneously, including every unrelated Gateway served by those processes.

## Current behavior (evidence)
- `add_gateway_contributions` returns `Err` for any Gateway whose resolved state is
  `ResolvedInboundProxyProtocol::Invalid { .. }` (`:395-397`), regardless of whether its listeners are
  admitted, and for any two contributors with different declarations on the same `ListenerKey`
  (`:418-430`). `aggregate_snapshot` short-circuits on the first error (`:355-363`).
- `GatewayHandler::full_set`/`partial_update` call `observe_snapshot` with **all** in-scope Gateways
  (unscoped processes include every non-dedicated Gateway of every namespace). Once a baseline exists,
  `Err(diagnostic)` or any non-equivalent snapshot calls `trip` (`:184-195`). `trip` is monotonic: a
  revert cannot clear it.
- After `trip`: `GuardFirstConnectionFilter::should_accept` returns `false` for every TCP service
  (`:551-554`), readiness becomes 503 (`api/mod.rs:92-98`) and `PolicyShutdownSignalWatch` requests
  FastShutdown (`cli/pingora.rs:161-170`).
- On restart, `GatewayBase::configure_listeners` runs `preflight_and_freeze(...)` and propagates the same
  aggregate error with `?` (`base.rs:82-85`), so startup fails with exit code 1 for as long as the
  offending Gateway exists. There is no "skip this Gateway" path.
- The Controller deliberately ships the `Invalid` state to the data plane instead of withholding it
  (`handlers/gateway.rs:671-690`, documented in `skills/01-architecture/05-resources/01-gateway.md`
  step 4: "always write Disabled/Ready/Invalid into the non-CRD transport field"). Reference failures
  (`ReferenceNotFound`, `ReferenceNotPermitted`, `ReferenceDisabled`) are ordinary transient states:
  deleting or disabling the referenced `EdgionConfigData`, or removing a ReferenceGrant, produces them.
- Shared-listener conflicts are symmetric: `conflicting_listener_names` reports the conflict for each
  contributor, so creating Gateway B on `:443` with a different (or absent) PROXY declaration marks the
  already-serving Gateway A `Invalid` too.
- Even a fully valid change is fatal: `equivalent_socket_semantics` compares the full declaration
  including `TrustedSources(Vec<String>)`, so rotating a load-balancer CIDR list trips every replica at
  once. The matcher is already an `Arc<dyn ProxyProtocolTrust>` (`prepare_policies`, `:331-346`) and
  Pingora consults it per connection, so trusted-source changes do not need a socket rebind.

## Industry comparison
- Envoy: an LDS update with an invalid listener filter config is NACKed per resource; the previous
  listener keeps serving, other listeners are unaffected, and a changed `proxy_protocol` listener filter
  is applied by draining the old listener and starting a new one (no process exit).
- Istio/Envoy and Traefik validate and reject per Gateway/entrypoint; a bad object degrades status, not
  the fleet. HAProxy `accept-proxy` / `tcp-request connection expect-proxy layer4 if { src -f list }`
  reloads trusted lists without dropping the process.
- No mainstream gateway turns a tenant-scoped object error into a coordinated fleet-wide exit.

## Impact
- Total data-plane outage across all replicas from one object edit, with no rolling behavior (all
  replicas observe the same conf_sync event within milliseconds) and a crash loop until a human finds and
  fixes the Gateway. Pod restarts cannot recover because startup applies the same fatal check.
- Blast radius crosses tenant boundaries: anyone who can create or edit any non-dedicated Gateway, or
  any referenced `EdgionConfigData`/ReferenceGrant, can trigger it.
- Routine operations (LB CIDR rotation, enabling PROXY on one new listener) require a planned full
  outage instead of a config push.

## Suggested direction
1. **Never let one Gateway's invalid state be process-fatal.** Treat `Invalid` and
   `SharedListenerConflict` as "this Gateway's TCP listeners are not admitted" (the Controller already
   reports `Accepted=False`/`Programmed=False`): exclude them from the aggregate and from listener
   publication, keep serving everything else. At startup, skip such listeners instead of failing.
2. **Make trusted sources hot-reloadable.** Back `CidrProxyProtocolTrust` with an `ArcSwap` of the
   matcher and publish new CIDRs without tripping; only the parse/no-parse boolean is socket semantics.
3. **For genuine socket-semantic changes** (Disabled ↔ enabled on a bound key), keep the old socket
   behavior and surface `Programmed=False` / a metric + readiness-neutral WARN until the next rolling
   restart, rather than self-terminating. If self-restart is retained as an option, it must be opt-in,
   jittered per replica and never applied to startup.
Recommendation: (1) + (2) immediately; (3) as the long-term model, consistent with the existing
"listener port set requires restart" constraint, which degrades instead of exiting.

## Notes
- Checked `skills/01-architecture/02-gateway/01-startup-shutdown.md` (§ inbound PROXY: documents the
  monotonic fatal guard and crash-restart intent) and `skills/04-review/listeners/` (no intentional-design
  page covers the fatal guard; `compatibility-constraints/listener-port-set-frozen-at-startup.md` covers
  port binding only). New evidence not considered by the documented design: (a) the guard also fires on
  `Invalid` / conflict states of unrelated Gateways, (b) startup applies the same check so the restart
  crash-loops, (c) all replicas trip simultaneously, (d) trusted-CIDR edits need no rebind.
- The unit test `invalid_state_fails_startup_with_bounded_code`
  (`edgion-gateway/tests/unit/runtime/server/proxy_protocol_policy_test.rs:354`) pins the current
  startup failure.
- Related but distinct: `gw-core-01` (shutdown/readiness ordering), `ctrl-proc2-01` (listener
  distinctness ignores protocol).

## Verification (independent)

- **Verdict**: CONFIRMED (P0 kept: one tenant's annotation error takes down every replica of every Gateway sharing the process, and restart cannot recover)
- `edgion-gateway/src/runtime/server/proxy_protocol_policy.rs:171-192` (`observe_snapshot`): once a baseline is frozen, any `aggregate_snapshot` error or non-equivalent snapshot calls `trip()` → `RejectFatal`; the doc comment states the change is intentionally "process-fatal".
- `:395-405` (`add_gateway_contributions`): a single Gateway in `ResolvedInboundProxyProtocol::Invalid` makes the whole aggregate fail; `:199-203` (`preflight_and_freeze`) runs the same aggregate at startup, so the next process start fails too.
- `edgion-controller/.../handlers/gateway.rs:246,412,684`: the controller does publish `Invalid` resolved states to the data plane.
- The fatal-on-semantic-change design is deliberate (listener sockets are frozen at startup), but escalating one Gateway's *invalid* state to a fleet-wide fatal is not justified by that rationale: the fix direction is to keep the last-good socket policy for that Gateway (status `Accepted=False`) instead of tripping, and to skip Invalid contributions at startup.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P0 unchanged)
- `edgion-gateway/src/runtime/` has no diff; `proxy_protocol_policy.rs` `observe_snapshot`/`preflight_and_freeze`/`add_gateway_contributions` unchanged, so one `Invalid` Gateway still trips the process-wide fatal and fails restart.
- Controller `handlers/gateway.rs` and `proxy_protocol_listener_policy.rs` have no diff.
