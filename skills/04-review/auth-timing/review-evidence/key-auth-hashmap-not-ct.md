---
name: key-auth-hashmap-not-ct
description: Use when reviewing key_auth constant-time comparison findings; explains why HashMap SHA-256 lookup is not a real timing attack surface and why a subtle::ConstantTimeEq switch is not accepted.
knowledge_type: review-evidence
---

# Key Auth HashMap Lookup is Not Constant-Time: Not a Real Attack Surface

## Conclusion

The `key_auth` plugin uses `HashMap<[u8;32], KeyMetadata>::get(&sha256_of(key))` to look up API keys; HashMap itself is not constant-time and `[u8;32]` comparison short-circuits. **This is not a real attack surface**; fix suggestions of the form "switch to linear scan + `subtle::ConstantTimeEq`" are not accepted.

## Reasons

1. **Input is first run through SHA-256**: the attacker controls the original `key_candidate` string; SHA-256 output is pseudorandom, so candidates with "controlled difference at the Nth byte" cannot be constructed for byte-by-byte timing inference
2. **HashMap timing leakage is minimal and worthless**: at most leaks "bucket-hit similarity of the first few bytes of sha256 output", but SHA-256 is one-way irreversible; this information does not help recover the original key
3. **The 401 response is already a binary oracle**: `validate_key` returns None → 401, returns Some → 200; "is the key in the table" can be inferred directly from the status code; the timing oracle does not add information for the attacker
4. **Classic byte-by-byte timing side channel does not hold**: SHA-256 preprocessing flattens input differences; the foothold for "change one byte and see how many more match" attacks does not exist
5. **Same approach in peer gateways**: mainstream API gateways like Kong, APISIX, Tyk, Traefik likewise use HashMap / etcd lookup for keys; none do CT linear scans

## Decision Rule

When a review/audit report contains the following patterns, close as "not a real issue":

1. "key_auth HashMap::get is not constant-time; timing side channel exists"
2. "Recommend `subtle::ConstantTimeEq` or linear scan over all hashes for CT comparison"
3. "[u8;32] == short-circuits, leaking number of matched bytes"

Verify two points before closing:

- `key_auth/plugin.rs`'s `validate_key` still uses `self.keys.get(&sha256_of(key))` (not linear scan)
- The input key is indeed first SHA-256ed (not changed to storing the plaintext key directly)

Both satisfied → not a real issue; point to the M10 NOT-AN-ISSUE decision in `tasks/security-audit/01-auth-plugins.md` and close.

## Exceptional Scenarios (need reassessment only under extreme threat models)

If in the future deployment occurs in a "the attacker can precisely measure local / sidechannel nanosecond timing" environment (e.g., public hardware / SGX / tenant-shared kernel), this conclusion needs reassessment. Normal K8s deployments do not fall into this category.

## Background Notes

- Decision date: 2026-04-21
- Decision source: `tasks/security-audit/` M10 entry
- Comparison file: `auth-timing/basic-auth-username-enumeration.md` (M4, also "mainstream does not do gateway-layer anti-timing protection"; but M4 has actual exploitability and goes doc-only; M10 has no actual exploitability and goes NOT-AN-ISSUE)
