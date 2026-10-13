---
name: auth-claim-header-projection
description: Use when reviewing JwtAuth/OpenID Connect claimsToHeaders, JweDecrypt payloadToHeaders, destination collisions, aggregate header caps, or array claim formatting.
status: fixed
finding_id: D0052
closed: 2026-09-11
knowledge_type: regression-guards
---

# Authentication Claim Header Projection Is Precompiled and Deterministic

## Conclusion and scope

JwtAuth and OpenID Connect `claimsToHeaders`, plus JweDecrypt `payloadToHeaders`, compile through
`compile_claim_header_projection` before request processing. The plan validates HTTP header names,
canonicalizes destinations to ASCII lowercase, rejects destinations that collide ignoring ASCII
case within one plugin execution path, and sorts entries by canonical destination. OIDC reserves
each enabled fixed output (`X-Access-Token`, `X-ID-Token`, `X-Userinfo`, and `Authorization`), while
JweDecrypt reserves its `forwardHeader`; a configured projection cannot claim the same destination.
Request processing consumes only that immutable plan, never the source `HashMap`.

This correction is local to each plugin instance. Separate plugins retain execution-order,
last-write-wins behavior. A missing, empty, unsupported, unsafe, or oversized value still skips its
write and does not clear a pre-existing destination header. D0052 does not establish Envoy-style
sanitize-before-rules behavior or reserve a global identity-header family.

## Rationale and evidence

Before D0052, `set_claims_headers` iterated the operator `HashMap` directly. Case variants such as
`X-Identity` and `x-identity` therefore competed for one HTTP destination with an unstable winner.
When aggregate capacity could not admit every candidate, `break` made the admitted subset depend
on the same unstable order. Array claims also materialized a `Vec<String>` and joined the entire
candidate before applying the per-value cap.

The shared compiler now lives in
`edgion-resources/src/resources/edgion_plugins/plugin_configs/auth_header_projection.rs`. All three
resource validators call it, so Controller and Gateway admission use one collision and header-name
contract. `JwtAuth`, `OpenidConnect`, and `JweDecrypt` compile and retain the resulting plan during
runtime construction. `edgion-gateway/src/plugins/http/common/auth_common.rs` formats arrays
directly into one bounded candidate and commits aggregate bytes only after a sensitive header write
succeeds. OIDC fixed token/userinfo headers retain priority and also commit their budget only after
a successful write.

## Alternatives and limits

ARCH-070: HeaderCertAuth's certificate source and four configured upstream header
names, plus HmacAuth's upstreamHeaderFields, use the canonical Resources HTTP token
check during shared Controller/Gateway admission. Existing empty/control checks
retain precedence; new diagnostics identify only the field or index. HMAC metadata
selection preserves exact whitelist spelling, and signed-header pseudo names are
outside this check. These producers do not adopt claim-projection normalization,
collision rules or sanitization: valid-name writes and skipped-write behavior remain
unchanged. This is character admission, not an additional header-length policy.

- Appending or joining competing claims into one header was rejected because the mapping expresses
  one source per destination; implicit multi-value semantics would change what an upstream
  authorization service receives.
- Sorting the `HashMap` on every request was rejected because configuration order is independent of
  request data and belongs in runtime construction.
- Cross-plugin destination ownership remains out of scope. Later plugins overwrite earlier writes
  through the existing replace-all-lines session contract.
- The deterministic aggregate policy is canonical-destination order. A candidate that cannot fit
  the remaining total budget stops later plan entries, preserving the former stop-on-cap shape with
  a stable admitted set.
- Claim paths remain case-sensitive. Only destination header names are canonicalized.
- Array projection preserves scalar filtering, source element order, comma formatting, and UTF-8
  byte accounting. It discards the entire candidate when the next append would exceed the cap; it
  never truncates.

## Verification and re-evaluation

Targeted resource tests cover canonical ordering, invalid destinations, and case-insensitive
mapping collisions through all three configuration validators, all four enabled OIDC fixed-output
collisions, and the JweDecrypt `forwardHeader` collision. Targeted Gateway tests cover exact and
plus-one per-value/aggregate boundaries, mixed array formatting, failed-write budget accounting,
sensitive lowercase writes in JwtAuth/OIDC/JweDecrypt, and JweDecrypt construction order.

Re-evaluate this guard if the configuration changes from one-source-per-destination mappings to an
ordered list with explicit append/merge semantics, if aggregate selection gains an operator-defined
priority, or if plugin composition adopts cross-plugin header ownership. Envoy-style clearing of a
configured destination when its claim is skipped remains a separate behavior decision; do not infer
it from D0052.

## Resolution references

- `D0052` — fixed 2026-09-11; the closed todo was migrated into this regression guard.
