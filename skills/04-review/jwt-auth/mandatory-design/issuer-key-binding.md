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
`dangerous::insecure_decode`, without authentication) BEFORE selecting a key.
Reject patches that:
- Iterate all groups trying each one's keys
- Fall back to a "default" group when iss doesn't match
- Look up kid before iss is known

## Invariant 2 — Algorithm-confusion defense (header.alg vs group.algorithm)

After routing, before key selection, the code must assert
`peek.header.alg == to_jwt_algorithm(group.algorithm)`. Without
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

## Unverified routing header reuse and scratch release

The insecure routing decode supplies both Header and claims. verify_jwt reuses
that header for the algorithm equality check instead of calling decode_header
separately. The historical jsonwebtoken 9.3.1 comparison established that both
paths split the token identically and used Header::from_encoded; its configured
peek disabled signature validation and the earlier signature-key/algorithm checks.
The current 11.1.0 untrusted API likewise returns both header and routing claims.
Header or routing-payload parse failure returns Malformed with the same fixed
diagnostic. A successful peek does not authenticate either field.

Once issuer routing, algorithm equality and scoped key selection succeed, the
peek claims and header are explicitly dropped before real decode. The selected
group/key references borrow the immutable store, not the lookup strings.
Previously the extra initial header was retained while the peek's unused header
was dropped; reusing the peek result removes that duplicate header parse while
retaining the established routing-scratch release boundary.

The insecure routing and real cryptographic decode stages remain distinct.
Signature verification consumes the original token with the selected group's key
and validation policy; only verified claims become the authenticated result.
The malformed-input/precedence regression retains fixed expectations captured
with the old prefix and exercises issuer-before-algorithm and algorithm-before-key
failures, empty-signature rejection and fixed failure-log redaction. Re-evaluate
this equivalence if the dependency or peek validation policy changes. No parser
bypass, physical-erasure or measured throughput claim follows from this change.

## jsonwebtoken 11 routing boundary

The workspace selects jsonwebtoken 11.1.0 with use_pem and the existing AWS-LC
backend. Rustls provider initialization remains independent. The untrusted stage
uses dangerous::insecure_decode with a private routing deserializer: require one
JSON object and reject duplicate exp, nbf, sub, iss and aud fields after JSON
name decoding. Other field names retain last-value-wins semantics. This stage
never authenticates claims or runs expiration/audience policy.

The dummy HMAC decoding key and disabled Validation formerly needed for routing
are removed. Prepared real credential keys, error caching and per-group
Validation remain. Exact issuer routing, configured algorithm equality, scoped
kid selection, scratch release and full verification of the original token keep
their existing order. Only the full decode's claims enter VerifyOk.

Do not copy the old SDK's private coercion/cursor quirks into Edgion. Some invalid
claim shapes can move from an early Malformed result to a later signed-decode
failure; every public denial remains generic invalid_token/401 and diagnostics
remain fixed. Non-object payloads consistently fail routing, including the old
five-element array edge that previously reached MissingIss. Optional exp/nbf
format validation follows the newer SDK, with required exp and nbf checks kept
intact. This does not claim every unused optional claim type is rejected.

Tests retain the existing failure-order/redaction expectations, add registered
and escaped-name duplicates, other-field last-value behavior and signed invalid
claim rejection. Fixed OpenSSL-key tokens signed and verified by the old 9.3.1
Ring backend exercise all eight supported issuer-group algorithms with the new
backend and reject altered signatures. These are compatibility/security guards,
not a measured performance improvement.
