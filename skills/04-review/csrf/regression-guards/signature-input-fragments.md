---
name: csrf-signature-input-fragments
description: Preserve CSRF signature bytes while avoiding a nonce-sized temporary signing buffer.
knowledge_type: regression-guards
---

# CSRF signature input fragments

## Conclusion and scope

`CsrfToken::gen_sign` feeds HMAC-SHA256 the existing message as ordered fragments:
`expires:`, standard signed i64 decimal text, `,random:`, and the nonce's exact
UTF-8 bytes. It does not assemble a second copy of the nonce into a signing
String. The integer text and final lowercase hex signature remain owned.

Both issuance and verification use this function. Changing the delimiters,
integer spelling, encoding, fragment order or nonce normalization would change
token signatures and is outside this optimization. Empty/negative/extreme
inputs retain the helper's behavior; this does not expand configuration or token
acceptance. Verification still enforces its existing lifetime and signature rules.

## Evidence and limits

The unit tests in `edgion-gateway/tests/unit/plugins/http/csrf/token_test.rs`
compare the result with the former complete-message implementation across signed
i64 boundaries, empty/ordinary/long nonce values, embedded delimiters, UTF-8
and short/long keys. The existing known signature and wire-shape checks remain.
The allocation comparison constructs its fixtures outside the measured closures
and includes complete signature generation on both sides; only lower requested
allocation bytes for the selected nonce lengths are asserted.

This removes the composite signing buffer, not all cryptographic allocations.
Multiple HMAC update calls replace one update; the allocation measurement does
not establish a CPU or throughput improvement. Key preparation, cryptographic
comparison, token encoding/decoding, randomness, expiration and logs are unchanged.

## Re-evaluation

Recheck byte parity and the owning token wire contract if signature construction,
integer formatting, HMAC algorithm or nonce encoding changes. Any cached key state
or token representation redesign needs separate ownership and security review.
