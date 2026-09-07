---
name: group-in-ok-log
description: Use when reviewing JwtAuth PluginLog assertions or fixture changes; documents the OK-line group=<g> iss=<iss> u=<sub> format and the rule that every failure branch must NOT include group/iss.
knowledge_type: mandatory-design
---

# JwtAuth — group + iss in OK access log, neither in failures

Per-plugin instance of the cross-feature [Access-log group attribution iron
rule](../../security/mandatory-design/iron-rules.md) — read it for the shared threat model. JwtAuth
extends the rule with `iss` / `kid` / `alg` (not just `group`) because each is a
separate topology-probing vector. JwtAuth specifics:

**OK line** — `OK group=<g> iss=<iss> u=<sub>`, always (`params.username` comes
from the `sub` claim; iss-routing guarantees group + iss are both known at OK
time). `iss` is logged **raw** (not via `sensitive(...)`) — it is a known partner
domain, e.g. `https://partner-a.example`.

**Failure branches** (must NOT carry `group=` / `iss=` / `kid=` / `alg=` values):

- `Deny malformed token`
- `Deny missing iss` — token has no iss claim (or non-string)
- `Deny unknown iss` — iss doesn't match any group (no iss value!)
- `Deny bad algorithm` — header.alg vs group.algorithm mismatch (no alg!)
- `Deny missing key` — kid not in routed group's credentials (no kid!)
- `Deny invalid credentials` — credential PEM/secret malformed
- `Deny bad signature` — real verification failed
- `Deny exp` / `Deny nbf` / `Deny bad aud` — claim validation failures
- `Deny exp too far` — maximumExpiration post-check
- (request_stage) `Anon=<name>` — anonymous bypass (no group concept)

**Why each value is withheld** (jwt-specific): leaking `iss` enumerates configured
issuers (probe via "unknown iss"); leaking `alg` maps an issuer's algorithm (probe
via "bad algorithm"); leaking `kid` maps key distribution across groups (probe via
"missing key"). Collectively they map the tenant topology.

## Implementation

- `edgion-gateway/src/plugins/http/jwt_auth/verify.rs` (8 Deny branches + 1 OK)
- `edgion-gateway/src/plugins/http/jwt_auth/plugin.rs` (OK log push on the
  `run_request` success path)

## Related

- `skills/04-review/jwt-auth/mandatory-design/issuer-key-binding.md` — iss-routing 4-invariant contract
- Shared rule + sibling plugins: [security/iron-rules.md](../../security/mandatory-design/iron-rules.md).
