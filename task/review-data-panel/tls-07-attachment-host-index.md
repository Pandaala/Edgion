# Evaluate prepared host lookup for exact TLS attachments

Status: deferred user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `41662e524`. No runtime implementation selected.

## Evidence

`tls/store/cert_matcher.rs::TlsCertMatcherSnapshot::match_attachment_sni`
looks up one exact ResolvedListenerAttachment, then scans its ordered certificate
candidates and each candidate's configured hosts using the canonical RFC 6125
matches function. An attachment group admits at most 64 candidates and runtime
EdgionTls validation admits at most 16 hosts per resource, giving at most 1,024
pattern checks per lookup. String-comparison cost still depends on input length.
The matcher itself allocates no temporary strings and compiles no regexes.

Publication in tls_store constructs the attachment bucket and sorts candidates
by creationTimestamp (None first), namespace and name. The handshake select
callback uses this query to find an EdgionTls overlay for the exact winning Gateway
listener when frontend mTLS is configured. Strict rejection occurs before this
query. Ordinary listener-SNI selection uses a different query and is not the
subject of this candidate.

## Semantic boundaries

The first matching candidate wins across all of its patterns. An older resource
with *.example.com can therefore beat a newer resource with api.example.com for
api.example.com. Replacing this with the existing exact-before-wildcard lookup
would change certificate selection. A prepared index must preserve resource
precedence across matching pattern classes, not simply return the exact bucket.

The canonical matches function and canonical_key/lookup have different input
validation behavior. The latter rejects domains that the former may compare
literally. Verify actual publication and SNI domains before narrowing acceptance;
do not infer equivalence solely from common DNS examples or introduce a second
hostname grammar. This is a source-level distinction, not a demonstrated invalid
certificate acceptance or trust bypass.

Overflow above 64 candidates omits the complete attachment overlay group, not a
truncated prefix or a per-host subset. Unrelated attachments must not shadow the
requested attachment. Preserve complete resolved proof identity, Gateway client-auth
authority, stale-proof rejection and dual-client-auth conflict handling. Keep one
captured immutable generation through selection; no second snapshot lookup or
mutable handshake cache is authorized.

## Alternatives and costs

- Retain the bounded scan if realistic candidate/host distributions favor its
  simple contiguous representation.
- Evaluate prepared per-certificate host data shared with its immutable compiled
  owner. This can reduce repeated pattern interpretation without duplicating
  per-attachment structures, but a small list may be cheaper than a hash index.
- Evaluate an attachment-local lookup retaining candidate precedence across exact
  and wildcard matches. Measure memory and incremental publication costs as well
  as handshake comparison counts; a resource may have up to 2,048 resolved
  attachments, so naive replication can outweigh lookup savings.

Any extension belongs under the existing canonical certificate matcher and
publication owner. Preserve full/partial update agreement, removal/replacement,
structural sharing for untouched state, retained old snapshots, last-known-good
server identity and separate trust-revocation behavior. Do not broaden caps,
precompute eligibility from a different generation, add request-path tracing or
change cryptographic verification to make an index easier to implement.

This is distinct from [routing-02](routing-02-listener-specificity-index.md),
which indexes listener specificity, and
[tls-06 validated SNI handoff](tls-06-validated-sni-handoff.md), which concerns
Route hostname parsing. It does not select a new index or authorize implementation.

## Acceptance before implementation

Compare current and candidate winners for exact/wildcard overlap with opposite
resource precedence, timestamp ties/missing timestamps, namespace/name tie breaks,
case variants, missing/malformed SNI, single-label wildcard depth and multiple
hosts per candidate. Exercise 64/65-candidate overflow and recovery, unrelated
attachments, changed host sets, replacement/removal, empty deltas, full versus
partial publication, retained snapshots, stale proofs and frontend mTLS conflicts.

Measure small/common and worst admitted groups, early hits, late hits and misses.
Report comparison counts, allocation/resident index size, update cost and lookup
CPU separately; existing tests do not establish performance. Use canonical RFC
6125 matcher, TLS store and tls_pingora tests, plus the original downstream TLS
architecture and frontend-validation ownership guidance. Freeze an exact IMPL
allow-list and obtain independent design/final reviews before a runtime change.
