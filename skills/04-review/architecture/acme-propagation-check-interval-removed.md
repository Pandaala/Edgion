---
name: acme-propagation-check-interval-removed
description: Use when reviewing EdgionAcme dns-01 propagation-wait findings; covers the propagationCheckInterval remove-then-reintroduce history and the authoritative-NS propagation poll loop semantics.
status: fixed
finding_id: acme-dns01-propagation-check-interval-dead-knob
closed: 2026-07-20
---

# ACME dns-01 `propagationCheckInterval` — Removed as Dead Knob, Then Re-introduced WITH Real Polling

## Conclusion

**History (two stages, both 2026-07-20):**

1. `spec.challenge.propagationCheckInterval` was originally deserialized,
   GEP-2257-validated, and documented but never consumed by any runtime code.
   It was removed (pre-release, no compatibility burden) under the
   remove-until-real principle.
2. The successor task (`acme-dns01-propagation-polling`) then implemented real
   propagation polling, and the field returned **together with the poll loop**
   — exactly as the removal record required. The dns-01 CRD block is
   passthrough, so no CRD YAML change was needed in either stage.

**Current behavior** (Phase 3 of `execute_dns01_flow`,
`edgion-controller/src/services/acme/service.rs` +
`propagation.rs` + `edgion-common/src/dns/txt_probe.rs`): the controller polls
the zone's authoritative nameservers (recursive resolution is used only to
discover the authoritative set — the converged cert-manager/lego design) at
`propagationCheckInterval` cadence (default `5s`, floor 1s) and activates the
challenges as soon as every TXT record is visible. `propagationTimeout`
(default `120s`, floor 10s) is a true upper bound. On expiry:

- a record **definitively observed missing** on a responding authoritative
  nameserver → best-effort TXT cleanup, then hard fail into the existing
  `failBackoff` retry path (activating would burn an ACME validation attempt;
  unanimous mainstream behavior);
- the check **never produced a definitive observation** (firewalled DNS
  egress, discovery failure) → WARN + optimistic proceed after the full
  timeout — equivalent to the legacy fixed wait, so restricted networks do not
  regress from "could issue" to "can never issue".

Re-reports of the form "propagationCheckInterval is a dead knob / never
consumed" are **not accepted** — the field is consumed by the poll loop.
Re-reports of "propagationTimeout is a fixed wait / always spent in full" are
**stale** — that was true only between the two stages above.

## Core Rationale

**1. Remove-until-real beats keep-and-lie**

Pre-release there is no compatibility burden, so the honest API surface is one
without the no-op knob. The field was re-introduced only together with a real
poll loop (the cert-manager / lego model), never before it — this principle
was honored: both stages landed the same day.

**2. Authoritative-NS check, recursive discovery only**

Recursive resolvers negative-cache the NXDOMAIN for
`_acme-challenge.<domain>` from before the record was created, giving false
negatives until the SOA negative-TTL expires. cert-manager and lego both
query the zone's authoritative nameservers directly for the real check; the
implementation (`edgion-common/src/dns/txt_probe.rs`) follows that model
using the same hickory-resolver backend as the shared resolver infra
(cross-cutting unification rule 4: no second DNS library).

**3. Timeout expiry distinguishes "confirmed missing" from "unverifiable"**

Mainstream tools hard-fail on timeout but all ship an escape hatch for
networks that cannot reach authoritative NS (cert-manager
`--dns01-recursive-nameservers-only`, lego `propagation-disable-ans`). Edgion
v1 ships no such knob, so the poll loop degrades instead: if no definitive
observation was ever possible, it waits out the full timeout and proceeds
optimistically (WARN) — the legacy behavior. A definitive "record absent"
observation at the deadline hard-fails into `failBackoff` after best-effort
TXT cleanup.

**4. Passthrough CRD means schema changes are YAML-free**

`AcmeChallenge` is an internally-tagged serde enum under a
`x-kubernetes-preserve-unknown-fields` CRD block; adding or removing the
field never requires a CRD YAML change.

**5. Probe egress is a recorded rule-3 exemption**

The probe dials NS IPs derived from DNS answers for operator-supplied
`spec.domains` with no `classify_ip` at dial time (cross-cutting rule 3).
Deliberate: private NS IPs are legitimate (internal DNS + internal ACME
server issuance — the same restricted-network scenario the optimistic
degradation protects), the gateway-crate classifiers are unreachable from
`edgion-common`, and the blast radius is bounded to outbound DNS queries
whose responses fold into a tri-state observation only. Re-reports of the
form "txt_probe is an unclassified egress path / internal port-53 scanning
oracle" are an accepted tradeoff, not a bug — see the rule-3 note in
`skills/01-architecture/00-common/05-unification-points.md`.

**6. `propagationTimeout` is a loose upper bound, not a hard wall-clock cut**

The deadline is evaluated between poll rounds; an in-flight round (per-record
budget `CHECK_ROUND_BUDGET` = 20s, sequential per record) is never
interrupted, and one final round deliberately runs at expiry. Worst case the
wait exceeds the timeout by ~N_records x 20s (only when nameservers hang —
healthy rounds finish in milliseconds). Cold path; accepted. Do not
re-report "poll loop can overshoot propagationTimeout" as a bug; a fix would
have to cancel the final observation round, trading a definitive answer for
punctuality.

**7. CNAME-delegated `_acme-challenge` was never supported end-to-end (not a
polling regression)**

A reviewer may claim the probe "fails closed on CNAME-delegated challenge
names that previously worked". False premise: Phase 2 creates the TXT via
the zone's own provider API at `_acme-challenge.<domain>`; a CNAME at that
name excludes coexisting TXT records (RFC 1034 node exclusivity — providers
reject the create, and even if shadow-created, a validating ACME server
follows the CNAME into the delegated zone where Edgion never writes). The
setup failed before this change too, just at a different phase. CNAME
support remains a feature request (see Re-evaluation Triggers), not a
regression.

## Fix Suggestions Not Accepted

- "Wire `propagationCheckInterval` into a single sleep as the wait duration"
  — wrong semantics; the interval is a poll cadence, not a wait.
- "Proceed optimistically on every timeout" — burns ACME validation attempts
  and rate-limit slots when the record is confirmed missing; optimism is
  reserved for the unverifiable case only.
- "Poll inside the resource worker" — ACME flows run in a spawned task, not
  the workqueue worker; workqueue Rule 5 (no sleeps in handlers) still holds.
- "Query recursive resolvers for the propagation check" — negative-caching
  false negatives; recursive is discovery-only.

## Re-evaluation Triggers

- A real-world report of the optimistic-proceed path masking a propagation
  failure (would motivate the explicit opt-out knob mainstream tools have).
- CNAME-delegated `_acme-challenge` support (lego follows CNAME chains; the
  probe currently does not).

## Reference Cases

- Origin task: `tasks/todo/acme-dns01-propagation-check-interval-dead-knob.md`
  (closed 2026-07-20 via Option B) and successor task
  `tasks/todo/acme-dns01-propagation-polling.md` (implemented and closed
  2026-07-20).
- GEP-2257 conversion that validated the field before removal:
  [gep2257-duration-field-scope.md](gep2257-duration-field-scope.md).
- Poll-loop unit tests: `edgion-controller/src/services/acme/propagation.rs`
  (`tests` module); probe pure-logic tests:
  `edgion-common/src/dns/txt_probe.rs` (`tests` module).
