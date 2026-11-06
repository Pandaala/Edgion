# ctrl-svc-03: ACME HTTP-01 activates challenges after a blind 5 s sleep instead of a readiness self-check

- **Category**: best-practice
- **Severity**: P2
- **Scope**: local
- **Location**:
  - `edgion-controller/src/services/acme/service.rs:1384` (`CHALLENGE_PROPAGATION_WAIT_SECS = 5`)
  - `edgion-controller/src/services/acme/service.rs:1434-1457` (patch tokens → sleep → `activate_http01_challenges`)
  - `edgion-controller/src/services/acme/acme_client.rs:236-271`

## Summary
Challenge tokens travel `EdgionAcme.status` patch → K8s watch → controller cache → conf_sync →
every gateway. The controller then sleeps a fixed 5 seconds and tells the CA to validate. Nothing
checks that the serving gateways actually have the token. If propagation takes longer (controller
under relist load, gateway reconnecting, a slow watch), the CA's validation fails, the authorization
and order become `invalid`, and the attempt consumes one of the five retry-budget slots (see
ctrl-svc-02) plus one of Let's Encrypt's "5 failed validations per account/hostname/hour".

## Current behavior (evidence)
`execute_http01_flow` patches `activeChallenges` (`service.rs:1436`), waits
`Duration::from_secs(CHALLENGE_PROPAGATION_WAIT_SECS)` (`:1446-1448`), then calls
`activate_http01_challenges` which calls `challenge.set_ready()` for every pending authorization
(`acme_client.rs:256-263`). There is no gateway acknowledgement, no HTTP probe, and the 5 s is not
configurable. Failure maps to `AttemptOutcome::Failed` with the 300 s x 4^n backoff.

## Industry comparison
- **cert-manager** performs a *self-check* before accepting the challenge: it repeatedly issues
  `GET http://<domain>/.well-known/acme-challenge/<token>` (through the same path the CA will use)
  and only calls the ACME "ready" endpoint once the expected key authorization is returned.
- **Traefik** and **Caddy** serve challenges in-process, so the token is live before activation
  by construction.

## Impact
Spurious validation failures under normal operational noise (rolling gateway restarts, large
config pushes), consumption of CA failed-validation rate limit and of the local retry budget, and
delayed issuance by at least 5 minutes per spurious failure.

## Suggested direction
Before `set_ready`, poll each challenge URL until it returns the key authorization (bounded, for
example up to 2 minutes with 2 s interval, cancellable by the attempt guard). Resolve the domain
the same way the CA would (public DNS) and use the egress-safe outbound client
(`build_outbound_client`, redirects denied). If the probe cannot reach the domain from inside the
cluster (hairpin NAT), fall back to verifying via the gateways' conf_sync acknowledged version of
the `EdgionAcme` resource rather than a fixed sleep. Keep the fixed delay only as a lower bound.

## Notes
Checked `skills/04-review/acme/*` (fencing, notification gating, gatewayRef serving ownership,
IPv6 host split); none discusses the propagation wait. Not tracked in `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P2
- `edgion-controller/src/services/acme/service.rs:1384`: `CHALLENGE_PROPAGATION_WAIT_SECS: u64 = 5`.
- `service.rs:1436-1453`: `patch_active_challenges` → `wait_or_cancel(5s)` → `activate_http01_challenges`; no probe or gateway acknowledgement.
- Interacts with ctrl-svc-02: each spurious failure consumes one of the five attempts.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2 (unchanged)
- `edgion-controller/src/services/acme/service.rs:1384,1446`: fixed `CHALLENGE_PROPAGATION_WAIT_SECS = 5` and `wait_or_cancel` before activation; file unchanged since `feature-08-30`.
- `acme_client.rs` unchanged; still no self-check probe or gateway acknowledgement.
