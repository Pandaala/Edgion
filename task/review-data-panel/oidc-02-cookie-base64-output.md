# Evaluate OIDC cookie Base64 output construction

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `e01516a44`.

## Evidence and decision

OpenidConnect::encode_signed_cookie_payload in
`edgion-gateway/src/plugins/http/openid_connect/session_cookie.rs` serializes the
payload, generates a 12-byte nonce and encrypts with AES-GCM. It then encodes the
nonce and encrypted bytes into separate Strings and formats nonce.ciphertext
into a third String. Both state and session cookies use this generic helper.

Decide whether to investigate a shared final output buffer. Fewer intermediate
owners alone do not establish a performance improvement. In the locked base64
0.22.1, Engine::encode calculates exact encoded length, allocates a zeroed byte
buffer, encodes into it and converts it into String. Engine::encode_string uses
ChunkedEncoder with a 1024-byte stack scratch buffer and 768-byte input chunks;
StringSink validates each encoded chunk as UTF-8 and copies it with push_str.
These implementations are in src/engine/mod.rs and src/chunked_encoder.rs of
that dependency.

Replacing the two encode calls and format with String::new plus encode_string,
push('.') and encode_string still copies all encoded bytes from scratch to the
final buffer and may repeatedly grow that buffer. It removes intermediate
owners, not necessarily allocations or copied bytes. Preallocating the output
or using safe encode_slice into a correctly sized byte buffer are alternatives
requiring checked sizing and a complete value assessment. No implementation or
benchmark has been produced; neither a speedup nor a slowdown is established.

## Boundaries and acceptance for later implementation

Keep serialization, random nonce generation, AES-GCM, URL_SAFE_NO_PAD encoding,
nonce.ciphertext framing, error mapping and caller cookie-size checks unchanged.
Keep the final owned String contract. Do not add unsafe code, a buffer pool,
shared mutable state, secret logging or a second cryptographic implementation.
Do not change token-cache writes, cookie publication or authentication behavior
as part of this output-construction investigation.

If approved, compare allocation events, allocated capacity, copied bytes and CPU
for representative state/session payloads, small values, encrypted input around
768-byte chunk boundaries, and near configured cookie limits. Test exact encoded
bytes using fixed nonce/ciphertext inputs and retain actual encrypted-cookie
roundtrips, tamper/error behavior and session persistence coverage. Measurements
must include output sizing and final String construction, not only the inner
encoder. Prefer a bounded local optimization only when evidence supports it.

Existing signed-cookie roundtrip and session persistence tests check decode and
access-token-cache behavior, but do not establish allocation or throughput
benefit. This documentation-only investigation ran no new tests or benchmarks.
