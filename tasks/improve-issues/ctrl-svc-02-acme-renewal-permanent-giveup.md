# ctrl-svc-02: ACME renewal permanently stops after ~7 hours of failures and lets the certificate expire

- **Category**: design-error
- **Severity**: P1
- **Scope**: local
- **Location**:
  - `edgion-controller/src/services/acme/service.rs:358-384` (`MAX_RETRY_ATTEMPTS = 5`, `exponential_backoff`)
  - `edgion-controller/src/services/acme/service.rs:756-793` (`check_resource`: same-generation budget → `Idle`)
  - `edgion-controller/src/services/acme/service.rs:880-921` (`run_attempt`: `next_delay = None` after the 5th failure)
  - `edgion-controller/src/services/acme/service.rs:613-618` (`AttemptOutcome::Failed(None)` sets no deadline)

## Summary
The per-generation retry budget (5 attempts, base 300 s, x4) is applied identically to first
issuance and to **renewal of a still-valid certificate**. With defaults the five attempts span about
7 hours (300 s + 1200 s + 4800 s + 19200 s). After the fifth failure the actor schedules nothing,
startup recovery re-reads `attemptsUsed=5` and returns `Idle`, and the only recovery paths are a spec
edit or the Admin trigger. A CA outage, an HTTP-01 path outage, or a rate-limit window lasting a few
hours during the 30-day renewal window therefore converts into a guaranteed certificate expiry
~30 days later, with no further automatic attempt.

## Current behavior (evidence)
- `run_attempt` on error: `next_delay = (attempts_used < MAX_RETRY_ATTEMPTS).then(...)`
  (`service.rs:886-887`); on the 5th failure it is `None` and `AttemptOutcome::Failed(None)` leaves
  `state.deadline` unset (`:615`).
- Restart/failover: `scan_all_acme_resources` → `check_resource(Startup)`; `persisted_attempts`
  keeps the same-generation count (`:847-854`), so `attempts_used >= MAX_RETRY_ATTEMPTS` → writes
  `Failed` and returns `Idle` (`:775-793`). No renewal deadline is ever re-armed.
- `needs_renewal` (`:856-873`) is only consulted when a check runs; no periodic "certificate is
  about to expire" re-check exists for a `Failed` resource.
- The phase moves to `Failed` even though the old certificate is still served, so the status gives
  no signal about time-to-expiry beyond `certificateNotAfter`.

## Industry comparison
- **cert-manager**: failed issuance backs off exponentially from 1 h, capped at 32 h, and
  **retries indefinitely**; a `Certificate` is never parked permanently while it is in its renewal
  window. Renewal time is `notAfter - renewBefore` and is re-evaluated continuously.
- **Traefik ACME**: renewal is re-attempted by its periodic (daily) renewal check for every
  certificate inside the renewal window, regardless of previous failures.
- **Caddy/certmagic**: retries with backoff up to a 1-day interval, continuing until expiry, and
  falls back to an alternate issuer.

## Impact
Silent, delayed outage: TLS for every domain on that `EdgionAcme` expires ~30 days after a transient
incident, long after anyone correlates it. This is the exact failure mode automatic renewal exists
to prevent. Large fleets with many `EdgionAcme` resources are hit together by a shared CA or ingress
outage.

## Suggested direction
- Keep the finite budget for **initial issuance** (protects CA rate limits for misconfigured
  resources), but for renewals (a currently valid certificate exists) never go idle: after the
  budget is used, keep retrying at a capped interval (for example `min(backoff, 24h)`, never later
  than `notAfter - emergency margin`).
- Alternatively, re-arm a deadline at `min(checkInterval, time-to-expiry/N)` from both
  `Failed(None)` and the startup `Idle` branch whenever `certificateNotAfter` is in the future,
  resetting the budget once per renewal window.
- Expose `edgion_controller_acme_certificate_expiry_seconds` (or reuse the existing cert-expiry
  gauge family) so operators can alert on time-to-expiry rather than on phase.

## Notes
- `skills/04-review/acme/regression-guards/acme-scheduler-fencing-recovery.md` and
  `skills/01-architecture/01-controller/08-acme-service.md` document the budget and the
  "edit or Admin trigger to reset" behavior, but only as a scheduler-fencing invariant; neither
  analyses the renewal case or accepts certificate expiry as a tradeoff. New evidence here: the
  budget is consumed by renewals of valid certificates, and the startup path cannot rearm it.
- `acme-service-shape.md` covers structure/SSA only.

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P1
- `edgion-controller/src/services/acme/service.rs:359-362`: `MAX_RETRY_ATTEMPTS = 5`, "stays in Failed until the user edits the CRD".
- `service.rs:886-887`: `next_delay = (attempts_used < MAX_RETRY_ATTEMPTS).then(..)` → `None` on the 5th failure; returns `AttemptOutcome::Failed(next_delay)`.
- `service.rs:766-772`: the renewal-not-needed `Wait` path applies only to `Ready`; a failed renewal sets phase `Failed` (`:899-903`), so `:786-800` writes exhausted-budget Failed and goes idle.
- `skills/04-review/acme/regression-guards/acme-scheduler-fencing-recovery.md` documents budget persistence for fencing, not acceptance of renewal give-up.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P1 (unchanged)
- `edgion-controller/src/services/` unchanged since `feature-08-30`; `services/acme/service.rs:362` `MAX_RETRY_ATTEMPTS = 5`, `:790` exhausted-budget idle path, `:886` `next_delay = (attempts_used < MAX_RETRY_ATTEMPTS).then(..)`.
- No acme review page added or changed that accepts renewal give-up.
