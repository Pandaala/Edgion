# Decide HeaderCertAuth connection projection ownership

Status: deferred for user decision. Scope: Connection-mode request projection,
not TLS certificate extraction or Header-mode trust verification.

## Evidence

`edgion-gateway/src/plugins/http/header_cert_auth/plugin.rs` calls
`session.client_cert_info().cloned()` in `extract_from_connection`. This clones
all fields of `ClientCertInfo` in `edgion-gateway/src/ctx.rs`: subject, every SAN
String and its Vec, optional CN and fingerprint. The clone detaches the snapshot
from the immutable session borrow before mutable header writes.

`apply_headers` uses only subject and first SAN when skip_consumer_lookup is on;
otherwise it uses the configured SAN-or-CN, CN or fingerprint identity. It then
writes the fingerprint separately. Later SANs are never read by this projection.
The original complete certificate info remains owned by the session and may serve
other consumers, so reducing this temporary snapshot must not truncate it.

Borrowing the whole certificate info through apply_headers is not a direct
replacement: immutable access through the session conflicts with its mutable
header setters. The current copy provides a real ownership boundary.

## Decision

Compare keeping the complete snapshot, copying only a private selected-field
projection, and preparing the bounded ordered sensitive HeaderValues while the
session is borrowed. Selected Strings can avoid unrelated certificate copies
but still need header materialization. Prepared values can avoid that intermediate
String owner but change preparation timing and require reuse of the canonical
sensitive-header mechanism. A new Vec of operations would add an allocation;
assess a bounded representation rather than assuming all projection plans are
cheaper. No broad Arc conversion of ClientCertInfo or session API redesign is
part of this candidate.

Measure complete callback allocation counts and bytes across zero, one and many
SANs, large subject/CN values, all identity strategies and skip-consumer mode.
Include success, missing identity, anonymous and header-conversion failures.
There is no measured throughput benefit or selected runtime design yet.

## Required acceptance

- Preserve the session's complete ClientCertInfo for all other consumers and
  retain original field bytes and sensitivity. No cache or persistent copy.
- SAN-or-CN uses the first SAN whenever present, including an empty String;
  fallback occurs only when the SAN list is empty. Subject is not a fallback.
- Missing identity and explicit CR/LF/NUL identity checks fail before any writes.
  Subject/SAN/fingerprint controls instead skip their respective writes.
  HeaderValue conversion rejects additional bytes; those errors are currently
  ignored and must not become an invalid-identity rejection or skip later writes.
- Preserve subject then SAN, or consumer identity, followed by fingerprint;
  preserve configured header collisions, ignored setter failures and anonymous
  header order. Keep shared sensitivity marking and canonical session mutation
  policy, including sealed RPC headers and deferred credential bookkeeping.
- Preserve anonymous allowance, rejection delay/challenge, fixed errors,
  PluginLog and Header-only credential hiding. No new tracing, PII output,
  schema, certificate trust policy or identity extractor implementation.
- Cover the above boundaries with targeted tests and independently review the
  selected ownership design and final diff before implementation acceptance.

Fresh baseline verification passed all 20 HeaderCertAuth tests. Existing identity
borrowing and subject-only denial/anonymous tests provide partial controls; they
do not establish allocation savings or all header-collision/option combinations.
This task changes no runtime code and does not claim a live TLS integration test.
