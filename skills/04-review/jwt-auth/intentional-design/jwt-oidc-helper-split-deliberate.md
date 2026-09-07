---
name: jwt-oidc-helper-split-deliberate
description: Use when reviewing "JWT/JWK parsing duplicated between openid_connect and jwt_auth / extract a shared JWT helper module" findings; covers why the split is a deliberate design, not fragmentation.
status: accepted-tradeoff
finding_id: jwt-helper-consolidation
closed: 2026-07-19
knowledge_type: intentional-design
---

# openid_connect vs jwt_auth JWT Helper Split — DESIGN-TRADEOFF

## Conclusion

The two JWT implementations — `edgion-gateway/src/plugins/http/openid_connect/jwt_common.rs`
(+ `token_verify.rs`) and `edgion-gateway/src/plugins/http/jwt_auth/{verify,credentials}.rs` —
implement **different validation models** and share almost no extractable logic. The split is
deliberate and stays.

Fix suggestions of the form "extract a shared JWT/JWK helper module under
`plugins/http/common/`", "unify the two `ErrorKind` matches", or "make jwt_auth reuse
`jwt_common`" are **not accepted**.

## Core Rationale

**1. The validation models are structurally different, not two copies of one model**

| Dimension | openid_connect | jwt_auth |
|---|---|---|
| Key source | JWKS via discovery, kid-miss forced refresh rotation | Controller-resolved static issuer-group credentials (K8s Secret, all in memory at construction) |
| Algorithm model | expected + allowed-list policy (`resolve_algorithm_policy`, incl. PS/EdDSA) | Exactly one algorithm per issuer group (`JwtAlgorithm` schema enum, HS/RS/ES only) — the alg-confusion defense, not a simplification |
| Key selection | `select_jwk`: kid + alg match inside a `JwkSet` | kid looked up only inside the iss-routed group's credential map |
| `DecodingKey` construction | `DecodingKey::from_jwk` | `from_secret` (with `base64_secret` branch) / `from_rsa_pem` / `from_ec_pem` |
| Verify flow | direct decode + issuer/aud/nbf validation | insecure-peek payload → iss routing → strict alg equality → group-scoped kid → real decode → `maximum_expiration` cap |

None of these rows can be served by one implementation without parameterizing away the
difference — i.e. re-creating both models behind a flag.

**2. The only textual overlap is the two `ErrorKind` matches, and merging them costs more than it saves**

`jwt_common::map_jwt_decode_error` returns `(u16, String)` user-facing HTTP responses;
`verify::classify_jwt_decode_error` returns `&'static str` PluginLog `"Deny ..."` tags
(a locked log-vocabulary convention — see the ldap-auth discriminated-error-log-tags rule)
and additionally handles `ErrorKind::InvalidAlgorithmName`. A shared helper would need a
third abstract error enum mapped twice — net more code, plus coupling two plugins'
independently-evolving error vocabularies.

**3. Below the registry's fragmentation threshold**

The unification-points registry (`skills/01-architecture/00-common/05-unification-points.md`)
records 2× duplication as an observation and extracts only at 3+ implementations (see the
"Known gaps & observations" section intro); this is 2, each plugin-internal (`jwt_common`
is `mod`-private to `openid_connect`; jwt_auth's helpers are `pub(super)` or tighter, with
`to_jwt_algorithm` `pub(crate)` but consumed nowhere outside the plugin). Neither leaks
outside its plugin, so there is no drift risk for third consumers.

## Fix Suggestions Not Accepted

- Extract a shared `plugins/http/common/jwt` module — the models differ on every axis; a shared module would be a parameterized union, more complex than the two originals.
- Unify `map_jwt_decode_error` / `classify_jwt_decode_error` — different output types and deliberately different message vocabularies (HTTP body vs PluginLog tag); merging couples them for ~15 lines each.
- Make jwt_auth consume `jwt_common`'s algorithm-policy helpers — jwt_auth's one-alg-per-group model is its algorithm-confusion defense; an allowed-list model would weaken it.

## Re-evaluation Triggers

Re-open this decision only if:

- A **third** JWT-verifying consumer appears in the gateway (crossing the registry's 3+ threshold).
- jwt_auth gains JWKS/remote-key support, making it actually need `Jwk`/`JwkSet` handling.
- Either plugin's helpers stop being plugin-private (exported for reuse elsewhere).

## Reference Cases

- 2026-07-19 unification inventory recorded the observation; investigated and closed same date (task `jwt-helper-consolidation`).
- `skills/01-architecture/00-common/05-unification-points.md` — "Known gaps & observations" entry.
- `skills/04-review/ldap-auth/review-evidence/discriminated-error-log-tags.md` — the PluginLog tag-vocabulary convention referenced above.

## Resolution references

- `jwt-helper-consolidation` — 2026-07-19 (⚠️ design tradeoff, no code change).
