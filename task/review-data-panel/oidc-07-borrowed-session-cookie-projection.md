# Evaluate borrowed OIDC session-cookie serialization

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `5b6ac242f`.

## Evidence and decision

The session request-filter refresh branches call `maybe_set_session_cookie_header`
in `edgion-gateway/src/plugins/http/openid_connect/token_exchange.rs`. It builds
an owned `OidcSessionCookie` projection solely for persistence, copying the
session reference and optional ID/refresh tokens. A nonblank access token is
already excluded without copying; whitespace-only tokens are still copied and
preserved. These transient strings remain alive through serialization and cookie
publication, while the caller retains the source payload.

Decide whether to replace this projection with borrowed serialization, and which
schema ownership to use. A separate Serialize-only view can borrow existing
strings but duplicates six field definitions, declaration order and omission
rules from `plugin.rs::OidcSessionCookie`. A generic owned/borrowed schema could
share those rules but broadens changes to decoding, refresh-result caching and
the redacted Debug implementation. Neither design is implemented or measured.
Keep the current implementation if its measured cost does not justify that
maintenance or implementation scope.

This concerns the plaintext projection before JSON serialization, separately
from [cookie Base64 output construction](oidc-02-cookie-base64-output.md).
Potential savings are the temporary field copies, not cache-owned strings,
JSON output, encryption buffers or the final cookie. Generating a missing
session reference still requires owned storage.

## Boundaries and acceptance for later implementation

Preserve the existing cache-before-cipher, encoding and cookie-size-check order,
including cache effects when later publication is skipped. Preserve reference
generation only for an empty reference, nonblank versus whitespace token
handling, exact token-cache contents and TTL, source immutability, and append
rather than replacement of Set-Cookie.

Keep exact plaintext field names, declaration order, optional values and
omissions. In particular, empty session_ref/access_token are omitted while
Some(empty) ID/refresh tokens are present. Preserve created_at and expires_at.
Do not modify random nonces, AES-GCM, framing, size caps, best-effort errors,
Gateway-cookie stripping, sensitive-data redaction or authentication behavior.
Do not add a cache, buffer pool or shared mutable serializer.

If approved, use IMPL and compare exact serialized plaintext with the current
owned representation across existing/generated references, normal/empty/
whitespace tokens, absent/present expiry and None/Some(empty)/nonempty optional
tokens. Retain actual encrypted roundtrips, source immutability and cache/
response-order coverage, including failed or skipped publication. Measure
allocation count, copied bytes and peak live storage at representative payload
sizes. Any zero-allocation assertion must be limited to constructing a borrowed
view of existing values, not the complete publisher.

The existing `persisted_session_cookie_preserves_fields_and_source` test covers
six token/reference combinations, and the original persistence regression
checks token exclusion and cache contents. They do not establish exact plaintext
serialization parity or the performance of a borrowed view. This documentation
batch ran no runtime tests or benchmarks.
