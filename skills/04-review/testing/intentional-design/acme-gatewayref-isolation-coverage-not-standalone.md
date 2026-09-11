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

A standalone blocked integration todo for HTTP-01 `gatewayRef` isolation is not
an independently actionable defect. The missing contract is D0026: Controller
validation, resolved serving attachment, Gateway dependency requeue, and
receiving-listener enforcement. Current challenge lookup is global by token and
Host only.

This closes D0027 and deletes
`tasks/todo/add-acme-gatewayref-isolation-integration-coverage.md`. It does
**not** close D0026. After that serving-attachment design is implemented, the
multi-Gateway HTTP-01 isolation scenario belongs on D0026.

Fix suggestions of the form "add the isolation integration scenario now",
"encode current global token/Host serving as the desired ownership contract",
or "keep a separate blocked D0027 todo" are **not accepted**.

## Core Rationale

**1. The tested ownership contract does not exist yet.**

`spec.challenge.gatewayRef` is serialized with `EdgionAcme` and reaches the
Gateway conf handler, but the Controller does not validate or resolve it into a
serving attachment or register Gateway dependency requeue. The Gateway store
indexes every synced challenge globally by token. Pre-route HTTP-01
interception in `try_serve_acme_challenge` matches Host/token before route or
Gateway selection. Writing the D0027 scenario now would either fail against
that global lookup or lock it in as desired product behavior.

**2. The missing coverage is D0026's acceptance, not a second finding.**

At that audit, ACME integration coverage was account and DNS-01 oriented. The DNS-01 cases were removed with the feature on 2026-09-09; account integration and HTTP-01 store tests remain. Source-local
store tests cannot prove Controller-to-conf-sync-to-listener ownership across
two Gateway identities. After D0026 lands, add one new dynamic scenario there:
two Gateway/listener ports share one challenge Host/token; selected A serves
and non-selected B falls through; a hot retarget from A to B reverses the
result; challenge removal or expiry makes both fall through. Reuse one existing
topology and readiness/`wait_phase` polling. Do not use fixed sleeps or edit
existing integration assertions.

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

- D0026 is implemented and still has no multi-Gateway HTTP-01 isolation
  scenario.
- Challenge serving starts enforcing target ownership while reviews still claim
  isolation coverage is a separate defect solely because this standalone todo
  is gone.
- A review claims ordinary single-Gateway HTTP-01 serving is broken solely
  because this standalone todo is gone.

## Reference Cases

- Active D0026 design task:
  [`acme-http01-gatewayref-serving-ownership`](../../../../tasks/todo/acme-http01-gatewayref-serving-ownership/acme-http01-gatewayref-serving-ownership.md)
- Closed D0027 / `tasks/todo/add-acme-gatewayref-isolation-integration-coverage.md`
- Store and intercept: `edgion-gateway/src/services/acme/challenge_store.rs`,
  `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs`
- Sibling close: [edgionplugins-lkg-identity-coverage-not-standalone.md](edgionplugins-lkg-identity-coverage-not-standalone.md)
- B0060: `tasks/pending/edgion-gateway-file-optimization/batches/B0060-acme-expiry-boundary.md`

## Resolution references

- `D0027` — 2026-09-06 (wont-fix).
