---
name: acme-propagation-check-interval-removed
description: Use when reviewing EdgionAcme dns-01 propagation-wait findings; covers the removed propagationCheckInterval dead knob and the fixed-wait propagationTimeout semantics.
status: fixed
finding_id: acme-dns01-propagation-check-interval-dead-knob
closed: 2026-07-20
---

# ACME dns-01 `propagationCheckInterval` Removed as Dead Knob — FIXED (field deleted)

## Conclusion

`spec.challenge.propagationCheckInterval` was deserialized, GEP-2257-validated,
and documented but never consumed by any runtime code (leftover since the
initial implementation, per git history). It was removed (2026-07-20,
pre-release, no compatibility burden) from `AcmeChallenge::Dns01`, the handler
validation list, the example YAMLs, and the docs tables. The dns-01 CRD block
is passthrough, so no CRD YAML change was needed.

`propagationTimeout` remains a **fixed wait** (`sleep(timeout.max(10s))` in
`execute_dns01_flow` Phase 3, `edgion-controller/src/services/acme/service.rs`)
— the docs and the in-code comment now say so explicitly instead of implying a
"max time" that polling would cut short.

Fix suggestions of the form "propagationCheckInterval is a dead knob / never
consumed" are **not accepted** — the field no longer exists. Suggestions to
"implement propagation polling so issuance doesn't always eat the full
timeout" are a tracked feature, not a review finding — see
`tasks/todo/acme-dns01-propagation-polling.md`.

## Core Rationale

**1. Remove-until-real beats keep-and-lie**

Pre-release there is no compatibility burden, so the honest API surface is one
without the no-op knob. The field is re-introduced only together with a real
poll loop (the cert-manager / lego model), never before it.

**2. Fixed wait is not a functional bug**

Slow propagation → ACME validation failure → `failBackoff` exponential retry
self-heals. Cold path, low frequency. The cost of the fixed wait is issuance
latency only, which the polling task tracks as a UX improvement.

**3. Passthrough CRD means residual YAML is harmless**

`AcmeChallenge` is an internally-tagged serde enum that ignores unknown
fields; an existing CR still carrying `propagationCheckInterval` deserializes
fine and the field is silently ignored — identical to its pre-removal
behavior.

## Fix Suggestions Not Accepted

- "Wire `propagationCheckInterval` into the Phase-3 sleep as the wait
  duration" — wrong semantics; the interval is a poll cadence, not a wait.
- "Keep the field with a deprecation note until polling lands" — pre-release,
  no compatibility shims by project policy.
- "Poll inside the resource worker" — ACME flows run in a spawned task, not
  the workqueue worker; workqueue Rule 5 (no sleeps in handlers) still holds.

## Re-evaluation Triggers

Re-open only when `tasks/todo/acme-dns01-propagation-polling.md` is
implemented — the field returns together with the poll loop, and
`propagationTimeout` docs revert to true-upper-bound wording.

## Reference Cases

- Origin task: `tasks/todo/acme-dns01-propagation-check-interval-dead-knob.md`
  (closed 2026-07-20 via Option B; successor task
  `tasks/todo/acme-dns01-propagation-polling.md`).
- GEP-2257 conversion that validated the field before removal:
  [gep2257-duration-field-scope.md](gep2257-duration-field-scope.md).
