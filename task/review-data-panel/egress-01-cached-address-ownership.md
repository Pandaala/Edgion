# Common egress cached address ownership

Status: deferred representation and value decision for the user.
Source baseline: 704481485. No runtime or dependency change.

## Evidence and scope

Gateway outbound clients and HTTPDNS bootstrap reach the canonical
`edgion-common/src/egress.rs::EgressResolver`. Its private memo owns Vec<IpAddr>
results after policy filtering. Inner::cached checks freshness under its read
lock before cloning the vector. Thus a fresh nonempty hit copies addresses while
holding that lock; stale hits do not copy. A successful DNS miss filters the
result, clones it into the memo, and retains the original for its caller.

The memo has at most 16 entries, not a demonstrated large-cache workload.
It caches empty filtered results as negative outcomes, but DNS failures return
before insertion. Zero TTL bypasses memo work. Store prunes expired entries,
then clears the whole map if a new key arrives at capacity. A replacement of an
existing unexpired key does not trigger that capacity clear. Keys are host text
within one resolver owner whose spec, policy, TTL and observer are fixed.

Reqwest Resolve consumes the allowed vector through a boxed owning iterator and
adds port zero to each result. The non-reqwest resolve_socket_addrs API returns
an independent Vec<SocketAddr> with the caller's port. Moving to shared IP storage
does not remove that latter output contract. Blocked errors retain different
adapter text: reqwest includes the queried host, while the socket API uses its
fixed generic error. The previous lazy error-formatting optimization remains.

This differs from [ExternalName DNS ownership](dns-01-address-result-ownership.md):
that candidate concerns a canonical LRU and a caller-specific first-safe scan.
Common egress returns the complete filtered list and has its own negative-result,
capacity and observer semantics. Do not copy an LRU redesign or selected-address
cache into this task.

## Alternatives and decision

- Retain independent vectors unless representative hit/miss measurements show
  a worthwhile improvement at this small entry bound and actual answer sizes.
- Share immutable Arc<[IpAddr]> results. Account for Vec-to-slice conversion and
  allocation, atomic handle operations and adapter iteration rather than assuming
  the original Vec allocation is reused.
- Share Arc<Vec<IpAddr>> results to preserve the original vector buffer. Account
  for the extra allocation/indirection and retained spare capacity, including
  small visible filtered results backed by larger vector capacity.
- Use an owned-or-shared result only where memo sharing pays, leaving zero-TTL
  and other unshared results owned. Weigh the extra representation and branches
  against avoiding unconditional Arc construction on every resolution.

A reqwest iterator must own its shared result until iteration ends; it cannot
borrow out of a released cache lock. Direct transports still need their owned
socket vector unless a separately reviewed API change is authorized. Do not hold
a lock across DNS await, transport await or consumer iteration. Outstanding
result owners can outlive eviction; the 16-entry memo bound is not a bound on
all retained response storage.

This task authorizes no new cache implementation, eviction policy, miss
coalescing, error backoff or public configuration. A canonical cache migration
would be a separate design decision, not a side effect of changing value storage.

## Acceptance for an approved implementation

Preserve full address order, duplicates, mapped-IP classification, empty-result
rejection and each adapter's error text/port behavior. Apply the existing policy
to fresh DNS results before insertion. Gateway rejection observation occurs
during that filtering, including each rejected address; cache hits currently do
not repeat it. Sharing must neither skip fresh filtering nor replay observation
for cached results. Keep DNS errors distinct and uncached.

Preserve resolver-owner isolation, exact host keys, zero-TTL bypass, strict TTL
freshness with saturating elapsed time, insertion timestamps, stale pruning,
replacement and capacity-clear behavior. Test ownership after eviction and
concurrent lookup/replacement, plus negative hits and safe/blocked mixed answers.
Never cache a caller-selected port under a host-only result key.

Compare allocation count, copied bytes, lock hold time, refcount contention,
retained capacity and total outstanding memory for empty/single/multiple-address
answers, hits/misses/expiry, repeated hot keys, many resolver instances and
zero-TTL resolution. Include reqwest iterator lifetime and direct socket-vector
materialization. No throughput or RSS claim follows from an isolated Arc clone.

Fresh unchanged-code verification passed 10 Common egress and three Gateway
egress tests. They cover existing policy/cache/adapter behavior, not a proposed
shared representation. No prototype, benchmark, full suite or live DNS/socket
test was performed.

Original knowledge:
[HTTPDNS egress](../../skills/04-review/ssrf/regression-guards/httpdns-endpoint-egress.md),
[Gateway observation](../../skills/04-review/observability/regression-guards/gw-plugin-outbound-01-egress-resolver-tracing-removed.md).
