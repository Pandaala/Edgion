---
name: issuer-key-binding
description: Use when reviewing JwtAuth verify.rs / credentials.rs / resolve_credential changes; documents the iss-routing security contract and the 4 invariants reviewers must enforce.
---

# JwtAuth — iss binds keys (security contract)

The JwtAuth `issuer_groups` design closes the pre-grouping vulnerability
"any group's kid validates any iss's token". After the grouping pilot, the
verify path enforces 4 invariants. Reviewers must reject patches that
break any of them.

## Invariant 1 — Route by iss FIRST

`verify_jwt` must look up the group by `iss` (peeked from payload via
`insecure_disable_signature_validation` decode) BEFORE selecting a key.
Reject patches that:
- Iterate all groups trying each one's keys
- Fall back to a "default" group when iss doesn't match
- Look up kid before iss is known

## Invariant 2 — Algorithm-confusion defense (header.alg vs group.algorithm)

After routing, before key selection, the code must assert
`decode_header(token).alg == to_jwt_algorithm(group.algorithm)`. Without
this, an attacker submits `alg=HS256` token claiming an RS-issuer's iss,
routes to the RS group, and the HMAC verification treats the public key
as a symmetric secret (classic JWT alg-confusion).

Reject patches that:
- Skip the equality check
- Use `Validation::new(header.alg)` for the real-verify step (must use
  `group.algorithm`)
- Allow `Validation.algorithms` to contain more than the group's algorithm

## Invariant 3 — kid scoped to the routed group

After routing to group, look up kid in `group.credentials` only — NOT in
any global map of all kids. Same kid can legitimately exist in two
different issuer groups with different secrets; cross-lookup is the bug.

Reject patches that build a flat `kid → cred` map across groups or that
fall back to other groups' credentials when kid is missing in this group.

## Invariant 4 — Verify with group.algorithm AND scoped cred

`build_decoding_key(group.algorithm, cred, config)` where `cred` came from
the routed group only. `Validation::new(group.algorithm)`. Both fixed by
the snapshot — gateway must not re-derive algorithm from any other source.

## Related

- `skills/04-review/jwt-auth/group-in-ok-log.md` — log format threat model
- `tasks/todo/plugin-grouping/03-jwt-auth-spec.md` §3, §6.2 — design record
- `tasks/todo/plugin-grouping/00-pilot-lessons.md` L7, L10 — failure-log
  topology protection + data-plane tracing iron rule
