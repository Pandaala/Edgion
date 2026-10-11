---
id: ARCH-381
status: confirmed
kind: architecture-debt
priority: P3
partition: PART-502
analyst: /root/analyze_outbound
baseline: 8f1c53149be48bb3de89f8cf43a7f08d7ecff602717ec079f6b7ee316705c306
---

# Construct an encoded, invalid signature for the OIDC signature-rejection probe

`test_jwks_invalid_signature_returns_401` in `edgion-tests/src/client/suites/edgion_plugins/openid_connect/openid_connect.rs:283` generates an RS256 JWT and changes its last encoded character to `a` (or `b`). The test then accepts any 401 as proof that a tampered signature was rejected. This construction does not preserve the token's encoding contract, so the test does not isolate the signature-verification boundary named by its identity.

The in-repository signing key and matching test-server JWK are a 2048-bit RSA pair. Its 256-byte signature has a 342-character unpadded base64url encoding. The last sextet therefore contains two data bits and four unused zero bits: canonical final characters are `A`, `Q`, `g`, or `w`. Both replacement characters have nonzero unused bits. The probe consequently creates a noncanonical encoding rather than guaranteeing a well-encoded, changed signature. A strict decoder can reject this before cryptographic verification; a decoder that ignores unused bits can instead retain the same final signature byte when its original high bits match the replacement. No fresh dependency execution or upstream decoder behavior is claimed here.

This distinction is material to the repository's authentication contract. `token_verify.rs` performs the real JWKS decode, and `error.rs:301–315` separately classifies base64/JSON/token-format errors and `InvalidSignature`; both project to 401. Auto mode permits fallback for the former and excludes the latter. The existing response-byte-cap review guard already documents correcting another OIDC fixture so it reaches the intended format-fallback branch rather than a different failure. The local and Kubernetes JWKS fixtures select `JwksOnly`, so this case's status-only assertion cannot distinguish the two boundaries. The adjacent valid-JWT case verifies the matching key and subject but does not repair this negative witness.

Keep the existing case identity and its 401 assertion, but construct the negative token by splitting the generated compact JWT, decoding the signature, flipping one signature byte, and re-encoding with the existing unpadded base64url codec. Preserve the original header and claims. Fail the probe if fixture decoding or reconstruction fails; optionally verify that the changed segment remains decodable and differs in decoded bytes. Mutating an early encoded sextet with an explicit changed-data-bit contract is a smaller alternative, although decoded-byte mutation makes the invariant clearer.

This is a bounded test-fixture correction, not a production authentication change. Preserve discovery, key selection, scope checks, error redaction, Auto fallback policy, existing fixtures, test identities, listener selection and transport behavior. No new logging, production visibility, generic JWT mutation framework, measured performance claim, or observed authentication bypass is proposed.
