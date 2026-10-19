# Decide JWE plaintext forwarding ownership

Status: deferred for user decision. Scope: the JWE request-filter plaintext
handoff to the forwarded sensitive header. No runtime change is proposed for
immediate implementation.

## Evidence

In `edgion-gateway/src/plugins/http/jwe_decrypt/plugin.rs`,
`decrypt_compact_jwe` decrypts the ciphertext Vec in place and transfers it into
String with `String::from_utf8`. `run_request` rejects header control characters,
then calls the shared `set_sensitive_request_header` with borrowed plaintext.
That helper in `plugins/http/common/auth_common.rs` constructs HeaderValue with
`from_str`, copying the bytes, marks it sensitive, and invokes the typed session
setter. The local plaintext remains available for optional claims projection and
`jwe_payload` context storage.

The callback order is forward-header insertion, claims projection, context
storage, credential removal, then the success log. A failed forward insertion
terminates before later consumers. JweDecryptConfig reserves forwardHeader in
compile_claim_header_projection, rejecting payload destinations that collide
ignoring ASCII case. The same plugin's claim projection therefore cannot overwrite
its forwarded header. A header-readback alternative still needs an explicit
lifetime design: borrowing text from the session conflicts with mutable claims
and context writes. It must also preserve non-ASCII UTF-8 access as discussed below.

Locked http 1.5.0 transfers an owned String through Bytes in
`TryFrom<String> for HeaderValue`. However, base64 0.22.1 `Engine::decode`
allocates a conservative decoded-length estimate and truncates the Vec to the
actual length. bytes 1.12.1 `From<Vec<u8>>` allocates Shared metadata when length
and capacity differ, retaining the original capacity. Exact-capacity storage
uses a different branch. Avoiding a payload copy alone does not prove fewer
allocations, lower retained memory or better throughput.

## Decision

Choose whether to keep the current independent header copy, transfer plaintext
only when neither later consumer needs it, or use a local shared owner that
survives all subsequent mutations. Measure allocation counts, copied bytes,
retained capacity and first-clone costs for realistic plaintext lengths,
including all Base64 tail lengths and every claims/context option combination.
Include complete callback costs, not only isolated conversion timings.

A shared-owner design must explain how the original UTF-8 text remains available.
HeaderValue::to_str accepts visible ASCII and tabs, whereas valid HeaderValue
bytes can contain non-ASCII UTF-8 accepted by the current path. It cannot replace
plaintext access without narrowing accepted input. Shrinking the buffer or
adding a second validation pass also has a cost to assess.

## Required acceptance

- Preserve forwarding bytes, UTF-8/control validation, failure precedence,
  authentication delay, fixed error responses and PluginLog outcomes.
- Preserve forward-header, claims, context and credential-removal ordering;
  preserve forwardHeader reservation and rejection of case-variant projection
  collisions. Cover setter failure, replacement of pre-existing destination
  values and original payload availability throughout later consumers. Do not
  move context writes before a failed forward-header insertion.
- Keep sensitive-header marking in the canonical shared auth mechanism and
  retain the typed session setter's sealed-RPC and deferred-credential rules.
- Preserve projection budgets and token-size limits, ignored invalid JSON,
  optional context behavior and credential hiding. Add no cache, persistent
  plaintext owner, new logging or schema change.
- Compare ordinary and non-ASCII plaintext, near-cap payloads, and both enabled
  and disabled optional consumers. Obtain independent plan/final review and
  required checks before any runtime change.

The existing JWE success-order test is useful baseline evidence, but does not
cover every option combination, setter failure or allocation tradeoff above. The
preceding runtime batch passed 21 JWE tests; this documentation task claims no
new runtime test execution or measured performance benefit.
