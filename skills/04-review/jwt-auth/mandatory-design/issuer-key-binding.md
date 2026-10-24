---
name: issuer-key-binding
description: Use when reviewing JwtAuth verify.rs / credentials.rs / resolve_credential changes; documents the iss-routing security contract and the 4 invariants reviewers must enforce.
knowledge_type: mandatory-design
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

- `skills/04-review/jwt-auth/mandatory-design/group-in-ok-log.md` — log format threat model
- `tasks/todo/plugin-grouping/03-jwt-auth-spec.md` §3, §6.2 — design record
- `tasks/todo/plugin-grouping/00-pilot-lessons.md` L7, L10 — failure-log
  topology protection + data-plane tracing iron rule

## Verified-result ownership evidence

VerifyOk owns the verified claims and borrows the selected immutable issuer group.
It no longer stores a second owned issuer String. The issuer accessor borrows the
validated nonempty string inside those claims; claims are private and exposed only
through a shared accessor. The sole production constructor retains issuer
validation before maximum-expiration checks and constructs the result only after
all checks succeed. The accessor's explained expect relies on this local invariant,
not on configuration admission or unverified peek data.

This is implementation ownership evidence, not an additional issuer/key security
rule. Routing, algorithm and scoped-key checks, signature verification, numeric
expiration checks and failure precedence are unchanged. Opaque Debug still hides
both metadata and claims. The success regression drops the original token and
compares the issuer slice with the claims field's pointer; the redaction fixture
includes an issuer sentinel in claims. Re-evaluate the accessor if construction
sites or mutable claims access expand. A claims lookup replaces the issuer copy;
no throughput improvement is measured.

## Unverified routing scratch release

After insecure decode, verify_jwt explicitly releases that TokenData's unused
header before moving out its claims. Moving only claims would leave the other
field owned until the original local's scope ends. Once issuer routing, algorithm
equality and scoped key selection succeed, it releases the peek claims and the
separate initial decode_header result before invoking real decode. The chosen
group/key references borrow the immutable store, not the lookup strings.

Both decode calls and all validation/error ordering remain unchanged. Signature
verification still consumes the original token with the selected group's key
and validation policy; unverified claims never become the authenticated result.
The plain owned parser fields have no custom cleanup behavior. This reduces
source-level overlap between routing scratch and real decode, not allocation
count or a measured whole-request peak/throughput; drop is not erasure.
