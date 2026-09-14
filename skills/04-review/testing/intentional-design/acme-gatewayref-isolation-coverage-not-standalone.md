---
name: acme-gatewayref-isolation-coverage-not-standalone
description: Use when reviewing ACME HTTP-01 gatewayRef isolation integration coverage, dual-Gateway challenge serving tests, or proposals to add a standalone D0027-style scenario before serving-attachment ownership exists.
status: wont-fix
finding_id: D0027
closed: 2026-09-06
knowledge_type: intentional-design
---

# ACME GatewayRef Isolation Coverage Is Not a Standalone Defect — WONT-FIX

## Conclusion

A standalone integration todo for HTTP-01 `gatewayRef` isolation is not an
independently actionable defect. D0026 now owns and implements Controller
validation, resolved serving attachments, Gateway dependency requeue, bounded
challenge publication, and receiving-listener enforcement.

This closes D0027 and deletes
`tasks/todo/add-acme-gatewayref-isolation-integration-coverage.md`. The
multi-Gateway HTTP-01 isolation scenario belongs to D0026 rather than a
second finding.

Fix suggestions of the form "add the isolation integration scenario now",
"encode current global token/Host serving as the desired ownership contract",
or "keep a separate blocked D0027 todo" are **not accepted**.

## Core Rationale

**1. The ownership contract must be tested at its D0026 owner.**

`spec.challenge.gatewayRef` is now resolved to Controller-owned listener
attachments. Pre-route interception validates the receiving listener and live
Gateway proof in addition to Host/token/expiry. Regression coverage must evolve
with that shared ownership contract, not as a detached D0027 task.

**2. The missing coverage is D0026's acceptance, not a second finding.**

At that audit, ACME integration coverage was account and DNS-01 oriented. The DNS-01 cases were removed with the feature on 2026-09-09; account integration and HTTP-01 store tests remain. Source-local
store tests alone could not prove Controller-to-conf-sync-to-listener ownership
across two Gateway identities. D0026 now combines Controller resolver and
service-gate tests, Gateway ownership tests, and a shared-topology dynamic
scenario with real config apply, conf-sync, and HTTP requests: selected A
serves, non-selected B falls through, retargeting to B reverses the result, and
challenge removal or expiry makes both fall through.

**3. Guessing serving-attachment shape is not a test substitute.**

D0026 still needs Gateway API-style reference validation and status,
cross-namespace ReferenceGrant dependencies, section-and-port semantics, and
atomic challenge indexing. A fixture that assumes one of those shapes would
encode an unapproved API and freeze the campaign's existing ACME files.

## Fix Suggestions Not Accepted

- Add the D0027 dynamic scenario before D0026 implements ownership — the
  assertions have no implemented contract and would encode a guess.
- Treat current global token/Host serving as the desired ownership contract —
  that is the D0026 defect, not accepted product behavior.
- Keep a separate blocked integration todo — it duplicates D0026 and is not
  independently actionable.
- Edit existing ACME integration assertions to manufacture this coverage — the
  campaign freeze and the original D0027 scope both forbid it.

## Re-evaluation Triggers

Re-open this decision only if:

- D0026's ownership tests stop covering selected/non-selected listeners,
  retarget/removal, or expiry.
- Challenge serving starts enforcing target ownership while reviews still claim
  isolation coverage is a separate defect solely because this standalone todo
  is gone.
- A review claims ordinary single-Gateway HTTP-01 serving is broken solely
  because this standalone todo is gone.

## Reference Cases

- D0026 regression guard:
  [ACME HTTP-01 GatewayRef serving ownership](../../acme/regression-guards/acme-http01-gatewayref-serving-ownership.md)
- Closed D0027 / `tasks/todo/add-acme-gatewayref-isolation-integration-coverage.md`
- Store and intercept: `edgion-gateway/src/services/acme/challenge_store.rs`,
  `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs`
- Related plugin-runtime decision: [EdgionPlugins key-scoped last-known-good](../../plugin-runtime/intentional-design/edgionplugins-key-scoped-last-known-good.md)
- B0060: `tasks/pending/edgion-gateway-file-optimization/batches/B0060-acme-expiry-boundary.md`

## Resolution references

- `D0027` — 2026-09-06 (wont-fix).
