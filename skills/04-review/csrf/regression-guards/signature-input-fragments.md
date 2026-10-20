---
name: csrf-signature-input-fragments
description: Preserve CSRF signature bytes while avoiding a nonce-sized temporary signing buffer.
knowledge_type: regression-guards
---

# CSRF signature input fragments

## Conclusion and scope

`CsrfToken::signature_bytes` feeds HMAC-SHA256 the existing message as ordered fragments:
`expires:`, standard signed i64 decimal text, `,random:`, and the nonce's exact
UTF-8 bytes. It does not assemble a second copy of the nonce into a signing
String. Standard signed decimal formatting writes the integer into a 20-byte
stack buffer through std::io::Write on a mutable slice; only the written prefix
enters HMAC. Even i64::MIN fits exactly, including its sign. Issuance uses `gen_sign` to encode the
32-byte digest as an owned lowercase hex signature. Verification encodes that
same digest into a fixed 64-byte stack buffer, removing its temporary hex String.

Both issuance and verification use this function. Changing the delimiters,
integer spelling, encoding, fragment order or nonce normalization would change
token signatures and is outside this optimization. Empty/negative/extreme
inputs retain the helper's behavior; this does not expand configuration or token
acceptance. Verification still enforces its existing lifetime and signature rules.
Its length check and XOR-fold compare the exact ASCII signature, preserving
uppercase and non-hex rejection. The fixed-size encoding assertion follows
hex 0.4.3's exact output-length contract: 32 input bytes require 64 output bytes.

After lifetime checks, verification rejects supplied signatures whose byte
length is not 64 before constructing HMAC or encoding its digest. This check
depends only on public input shape. Every 64-byte input still receives the same
HMAC calculation and complete XOR fold; no claim about nonce secrecy is needed.

## Evidence and limits

The unit tests in `edgion-gateway/tests/unit/plugins/http/csrf/token_test.rs`
compare the result with the former complete-message implementation across signed
i64 boundaries, empty/ordinary/long nonce values, embedded delimiters, UTF-8
and short/long keys. The existing known signature and wire-shape checks remain.
Verification tests also reject short, long, uppercase and non-hex signatures.
The allocation comparison constructs its fixtures outside the measured closures
and includes complete signature generation on both sides; only lower requested
allocation bytes for the selected nonce lengths are asserted.

A digest-only allocation test also covers i64::MIN, negative one, zero, one,
an ordinary epoch and i64::MAX with a long nonce. Its fixture and expected
signature are constructed outside the measured closure; the digest operation
allocates zero bytes. This is not a claim about token generation/encoding.

This removes the composite signing buffer and timestamp String, not all issuance allocations.
Multiple HMAC update calls replace one update; the allocation measurement does
not establish a CPU or throughput improvement. Key preparation, cryptographic
comparison, token encoding/decoding, randomness, expiration and logs are unchanged.

## Re-evaluation

Recheck byte parity and the owning token wire contract if signature construction,
integer formatting, HMAC algorithm or nonce encoding changes. Any cached key state
or token representation redesign needs separate ownership and security review.
