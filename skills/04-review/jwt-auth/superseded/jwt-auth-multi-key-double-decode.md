---
name: jwt-auth-multi-key-double-decode
description: Inactive historical payload-selector decision, replaced by issuer-scoped key selection.
knowledge_type: superseded
---

# Historical JWT payload-selector decision

Inactive: P3-g3-017 was closed as a design tradeoff on 2026-05-15 for the former
`Credentials::Multi` and `verify_token` branches.
That implementation selected credentials using a payload claim and decoded again
for verification. The configurable payload `key_claim_name` still exists; only
the old store/branch shape has been replaced. Blanket historical instructions to
close findings without re-evaluation are inactive.

The current `verify_jwt` first peeks `iss`, routes to one issuer group, checks its
configured algorithm, then reads the key selector from the payload field named by
`key_claim_name`, selects that key only inside the group and verifies the token.
It does not select keys from JWT header `kid`. See the
[issuer/key binding contract](../mandatory-design/issuer-key-binding.md)
and `edgion-gateway/src/plugins/http/jwt_auth/verify.rs`. Unverified routing data
must never become authenticated identity before verification.

D0036 source inspection identified this stale guidance. Two decode stages still
have different purposes; performance proposals require current evidence and must
preserve issuer scoping, algorithm binding, configured payload-selector semantics,
and malformed-token classification. Moving selection to header `kid` would be a
product behavior change, not an equivalent copy optimization.
Historical ecosystem alignment and pre-release compatibility claims do not justify
rejecting new findings without investigation. This documentation correction changes
no JWT behavior and makes no new performance measurement claim.
