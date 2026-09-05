---
name: csrf-expires-integer-seconds-retained
description: Use when reviewing CSRF expires integer-vs-GEP-2257 duration-string findings; covers why the positive integer-second lifetime stays and why B0070 does not reopen the closed duration scope.
status: accepted-tradeoff
finding_id: D0041
closed: 2026-09-05
---

# CSRF `expires` Stays Positive Integer Seconds — ACCEPTED-TRADEOFF

## Conclusion

`CsrfConfig.expires` remains a positive integer-second token lifetime. B0070 already
enforces that bound at schema validation and at runtime; the representation is not
part of that security property. The closed seven-field GEP-2257 conversion set is
not reopened for this existing plugin field.

Fix suggestions of the form "migrate CSRF `expires` to a GEP-2257 duration string
for consistency / because new durations default to strings / because B0070 touched
expiry" are **not accepted**.

## Core Rationale

**1. B0070 is correctness, not unit-ergonomics evidence.**

Nonpositive `expires` used to disable server-side expiry, and attacker-controlled
decoded timestamps could overflow subtraction. Those defects are closed by
`expires > 0` at `CsrfConfig::validation_error` and fail-closed checked arithmetic
in `CsrfToken::verify_at`. A string `"0s"` would need the same positivity check.
Cookie `Max-Age` is still not a server-side replay limit; that is why the integer
bound stays load-bearing and why `<= 0` remains rejected.

Sources: `edgion-resources/src/resources/edgion_plugins/plugin_configs/csrf.rs`,
`edgion-gateway/src/plugins/http/csrf/{plugin.rs,token.rs}`.

**2. None of the GEP-2257 re-evaluation triggers fire.**

The closed scope in
[gep2257-duration-field-scope.md](gep2257-duration-field-scope.md) reopens only
for a newly added CR-surface duration, a third-party anchor that itself moves to
strings, or operator feedback that a retained integer field is a recurring
misconfiguration source.

CSRF `expires` already existed at the 2026-07-19 conversion and was left with the
other plugin integer lifetimes (OIDC `sessionLifetime`, JWT `lifetimeGracePeriod`).
Trigger 1 applies to **new** fields, not to revisiting old ones. The shared
`parse_duration()` helper is the unified parser for fields that *are* strings; it
is not a mandate to convert every remaining integer. No operator report shows
`7200` being written as the wrong unit.

**3. Industry peers do not require a duration-string CSRF TTL.**

Edgion CSRF is the APISIX double-submit model (`name` default `apisix-csrf-token`,
`expires` default `7200`). APISIX keeps `expires` as a number of seconds and
allows `0` to skip expiry; Edgion is stricter. Envoy and Linkerd have no token
CSRF. Kong has no first-party CSRF token plugin; its session lifetimes are
integer seconds. Traefik Hub binds CSRF to the OIDC session and exposes
`session.expiry` as integer seconds, not `csrf.expires`. oauth2-proxy's
`--cookie-csrf-expire` is OAuth-state CSRF, a different problem.

Expiry here is a secondary stolen-token window on a **stateless, session-unbound**
token. GET/HEAD/OPTIONS reissue a cookie, so active browsers rarely hit the bound.
That does not justify deleting the field in this finding, nor converting it to
`"2h"`. Removing the operator knob or binding CSRF to a login session is a
separate design task.

## Fix Suggestions Not Accepted

- Convert `expires` to a GEP-2257 string — no documented trigger; schema, Gateway
  construction, EN/ZH docs, and integration fixtures would have to change
  atomically, with no int/string dual-accept or compatibility alias.
- Treat B0070 as authorization to migrate representation — the security property
  is "positive finite lifetime", already enforced on the integer.
- Dual-accept integers and strings, or add a deprecated alias — pre-release
  policy forbids compatibility shims.
- Delete `expires` or hard-code 7200 in this close-out — that is a wider public
  schema change, not required to reject the string migration.

## Re-evaluation Triggers

Re-open this decision only if:

- CSRF is redesigned to be session-bound and the standalone lifetime is removed
  or replaced by that session field.
- Operator feedback shows the integer-second `expires` field is a recurring
  unit-misconfiguration source.
- A later explicit design task authorizes an atomic representation change after
  the frozen integration-test boundary, still without dual-accept.

## Reference Cases

- Closed `tasks/todo/migrate-csrf-expires-to-duration-string.md` and campaign
  D0041 on 2026-09-05 without a runtime change.
- Parent duration-scope precedent:
  [gep2257-duration-field-scope.md](gep2257-duration-field-scope.md).
- B0070 correctness batch: commit `65511d86`
  (`tasks/pending/edgion-gateway-file-optimization/batches/B0070-csrf-expiry-runtime-ownership.md`).
- Sibling D0042 (Secret-rotation integration coverage) remains independent and
  is not closed by this decision.
