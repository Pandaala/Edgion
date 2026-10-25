# Decide outbound SAN fallback reason storage

Status: deferred for user decision. No runtime change.
Source baseline: abed3baed.

## Evidence and tradeoff

`edgion-gateway/src/tls/validation/outbound_san_verifier.rs::probe_fallback`
renders one error node with `to_string`, finds the first `SAN mismatch: ` marker,
and copies the entire remaining suffix into a separate String. The returned
owner requests storage for the suffix rather than retaining the whole rendered
error's capacity. An empty suffix need not allocate.

Removing the prefix in place and returning the rendered owner could avoid the
separate nonempty suffix allocation and copy into that allocation. It still
shifts suffix bytes and can retain capacity sized for a much larger rendering.
Shrinking or conditional compaction adds work and needs its own evidence.
Removing text from a String is not secure erasure of its allocation.

The ordinary typed SAN rejection bypasses this fallback. Its static reason is
already borrowed, and typed dynamic reasons are borrowed from their error owner.
The opportunity here concerns foreign errors recognized only through Display;
there is no demonstrated hot-path bottleneck or measured throughput/RSS benefit.

OIDC uses `outbound_san_mismatch_reason(...).is_some()` for classification and
discards the result. LDAP moves an owned fallback result through `into_owned`
into `LdapAuthError::TlsSanMismatch`, then explicitly drops it after diagnostics
and metrics and before awaiting the error reply. Do not assume both consumers
retain the allocation for the same lifetime.

## Decision and acceptance

Keep the suffix copy, or authorize a measured design for reusing the rendered
owner, optionally with a justified capacity policy. No alternative is selected.
This is separate from certificate SAN extraction and prepared policy matching.

- Preserve the complete typed-first pass, existing `MAX_CHAIN_DEPTH` of 16,
  typed `io::Error::get_ref()` replacement semantics, source-only fallback walk,
  and the depth-limit behavior that prevents fallback after typed exhaustion.
- Preserve the first marker, exact suffix bytes, empty suffix, repeated markers,
  Unicode boundaries and wrapper text. Do not change Display invocation count,
  order or failure behavior, or add an unrelated parser or truncation limit.
- Preserve classification, diagnostic text, redaction policy, metric ownership
  and LDAP's explicit release before response I/O. No raw-error logging or new
  observability is authorized by this storage task.
- Compare short/long prefixes and suffixes, empty/no-marker cases, spare rendering
  capacity, temporary peaks and retained capacity for each consumer. Separate
  allocations avoided from bytes moved; do not assume shrinking is free.
- Use IMPL with an exact allow-list and independent review after selection.
  Existing typed/recovery tests establish current semantics, not acceptance of
  a different storage policy or complete coverage of these proposed cases.

Fresh existing tests passed: outbound verifier 12, LDAP 28 and OIDC dependency
errors 8. Formatting, workspace all-target check/Clippy and agent-doc validation
passed with existing warnings. No runtime/test edits, full suite, live network
test, allocation experiment or performance benchmark are part of this task.
