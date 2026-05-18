---
name: group-in-ok-log
description: Use when reviewing JwtAuth PluginLog assertions or fixture changes; documents the OK-line group=<g> iss=<iss> u=<sub> format and the rule that every failure branch must NOT include group/iss.
---

# JwtAuth — group + iss in OK access log, neither in failures

After the JwtAuth `issuer_groups` rollout, successful verification writes:

- `OK group=<g> iss=<iss> u=<sub>` — always (JwtAuth's `params.username`
  comes from `sub` claim; iss-routing guarantees group + iss are both known
  at OK time)

Failure branches intentionally **do not** carry group or iss values:

- `Deny malformed token`
- `Deny missing iss`     — token has no iss claim (or non-string)
- `Deny unknown iss`     — iss doesn't match any group (no iss value!)
- `Deny bad algorithm`   — header.alg vs group.algorithm mismatch (no alg!)
- `Deny missing key`     — kid not in routed group's credentials (no kid!)
- `Deny invalid credentials` — credential PEM/secret malformed
- `Deny bad signature`   — real verification failed
- `Deny exp` / `Deny nbf` / `Deny bad aud` — claim validation failures
- `Deny exp too far`     — maximumExpiration post-check
- (request_stage) `Anon=<name>` — anonymous bypass (no group concept)

## Threat model

- **Success line carries `group=<g> iss=<iss>`** — caller already
  authenticated for that group; iss is a known partner domain (e.g.
  `https://partner-a.example`). Surfacing both in PluginLog is operator
  attribution for audit. iss is logged **raw** (not via `sensitive(...)`).
- **Failure line MUST NOT carry group/iss/kid/alg values** — an
  unauthenticated attacker must not be able to:
  - Enumerate which issuers are configured (probe via "unknown iss")
  - Map which algorithm a given issuer uses (probe via "bad algorithm")
  - Map kid distribution across groups (probe via "missing key")

  Each information leak narrows the attack surface; collectively they map
  the tenant topology.

Reviewers must reject patches that add `group=`, `iss=`, `kid=`, or
`alg=` literals to any `Deny *` branch.

## Implementation

Verify code lives in:
- `src/core/gateway/plugins/http/jwt_auth/verify.rs` (8 Deny branches + 1 OK)
- `src/core/gateway/plugins/http/jwt_auth/plugin.rs` (OK log push around the
  run_request success path)

## Related

- `skills/04-review/jwt-auth/issuer-key-binding.md` — iss-routing 4-invariant contract
- `skills/04-review/key-auth/group-in-ok-log.md` — same convention for KeyAuth
- `skills/04-review/hmac-auth/group-in-ok-log.md` — same convention for HmacAuth
