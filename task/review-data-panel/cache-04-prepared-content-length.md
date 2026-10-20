# Evaluate prepared response-cache Content-Length

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `1b2b4bcb9`.

## Evidence and decision

`edgion-gateway/src/response_cache/storage.rs::Store::lookup` validates and
decodes a cache entry, removes Transfer-Encoding, then inserts Content-Length
using `entry.used.to_string()`. Each successful lookup formats the same used
body length for that immutable published `Arc<Entry>`. Pending capture updates
used until completion; publication requires complete, current and fresh state.

Decide whether preparing the decimal value for repeated hits justifies its
extra persistent representation and memory accounting. Candidates include an
owned prepared HeaderValue, a prepared string, or bounded stack formatting at
lookup. A borrowed prepared string still needs header-owned storage; a shared
HeaderValue has its own capacity and owner-lifetime costs. Keeping the existing
construction may be preferable when entries have few hits.

The locked http 1.5.0 numeric HeaderValue conversion uses an itoa formatter but
also reserves maximum-width BytesMut storage before freezing. Direct insertion
of usize is therefore not automatically an allocation removal. The historical
u64 Age measurements in [cache-02](cache-02-age-header-construction.md) motivate
measurement, not a numeric result for this different value range and type.
No candidate implementation or performance measurement has been made here.

## Boundaries and acceptance

Any persistent field or backing owner must have an explicit memory ledger
justification. Admission precedes allocation, and charges must survive retained
readers, slices, eviction and replacement. Do not assume the ENTRY_CONTROL
constant grants unaccounted allocation headroom. Include pending refusal,
publication, cancellation and final release. Keep body callbacks allocation-free;
prepare only against the final used length under a reviewed ownership design.

Preserve complete metadata decoding and integrity validation before replay,
current-identity corruption eviction, freshness/epoch checks, hit header/reader
leases, Transfer-Encoding removal, Content-Length replacement, decimal bytes,
sensitivity and precommit failure behavior. Do not change Age construction,
cache eligibility, final response processing or transport framing policy.

If approved, use IMPL and compare repeated-hit CPU, allocation calls/bytes and
retained capacity against miss/low-hit entries at representative sizes. Test
zero, one, decimal-width transitions and configured maximum body length;
complete versus incomplete capture; exhausted budgets; failed decode; eviction,
replacement, retained readers and cancellation. A derived length representation
must agree with final used even under corruption fixtures, including an altered
used value still within arena bounds. Test exact H1/H2 replay framing, not just
the constructed header value. Preserve the current cache policy fixture issue
recorded in [verification-01](verification-01-cache-policy-fixture.md) without
treating historical failures as fresh validation.

Relevant ownership authority:
`skills/04-review/http-body/regression-guards/bounded-response-cache-capture.md`.
This documentation-only investigation ran no runtime tests or benchmarks.
