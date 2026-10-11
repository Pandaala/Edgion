---
id: ARCH-019
status: candidate
kind: architecture-debt
priority: P3
primary_partition: PART-283
independent_verification: confirmed
---

# Borrow resolved GeoIP dimensions when projecting an access log

## Existing owner and avoidable copies

observe/access_log/entry.rs GeoSlotLog::from_lookup clones country, continent,
subdivision and city Strings from GeoLookup::Found's Arc<GeoInfo>. Up to two
resolved slots can project those fields. AccessLogEntry already borrows its
EdgionHttpContext for its lifetime; RequestInfo::resolved_geo returns loans from
context-owned OnceLock slots without forcing a lookup. Terminal logging serializes
the entry to an owned JSON String before handing it to the async sender. Connection
establishment and debug-header callers also serialize while the source context lives.
No sender retains GeoSlotLog as an independent owned record.

The source comment partly justifies ownership by permitting short-lived temporary
test lookups. Production does not require that lifetime: its borrowed entry keeps
the context, and therefore the captured GeoInfo Arc, alive. This adds owned text
copies to the data-plane projection solely to make the local projection detached.
Adding another resolved string dimension would add another copy rather than a
borrow from its established owner. At most eight current dimension String clones
are avoidable when both found slots have all four fields; no measured throughput
or latency gain is asserted. Requests with no resolved slots still pay none.

## Bounded improvement and alternatives

Carry borrowed dimension strings in lifetime-bound GeoSlotLog/GeoLogBlock and
propagate the existing AccessLogEntry lifetime. Borrow the captured GeoInfo through
GeoLookup rather than cloning its Arc or creating a second owned fact. Preserve
the coordinate formatting and canonical masked serializer; that computed String
has a distinct purpose and need not be removed by this change.

Update tests to bind their GeoLookup values before constructing a projection.
Do not retain production copies for temporary fixture convenience. A custom
serializer could also borrow the lookup directly, but changing the whole wire
projection is unnecessary if the existing derived shape can carry references.
Retaining small copies is simple but provides no independent production ownership.

## Invariants and verification

Keep all statuses, dimensions, omission rules and coordinate masking unchanged.
Logging must continue to use OnceLock::get, perform zero new MMDB lookups and
allocate no optional request geo state. Captured facts remain valid across provider
reload. Intermediate entries still exclude terminal-only WAF details, and ext,
trace, async sink ownership and logging timing remain separate.

Use existing found/absent/failure/coordinates tests and actual-context projection
wiring. Check reference identity for string dimensions where useful, JSON equality
including escaping, lazy unset slots and old captured facts after provider swap.
Rust lifetimes must prevent invalidating slots while an entry borrows them. No new
visibility, cache, schema switch, metric or request diagnostic is needed.

## Original authority and limits

The coordinator fully read all five access-log primaries, actual logging/debug/
connection callers, RequestInfo's resolved facts, GeoInfo ownership, entry tests
and the original GeoIP lazy-slot decision. That decision requires non-forcing
projection and status distinctions; it does not require dimension ownership.
The project explicitly prioritizes data-plane ownership over mock convenience.
Backfill first-healthy routing, best-effort corrupt lines, blocking admin IO and
queue-entry completion are separately accepted tradeoffs and not part of this
finding. Source-only review; independent verification remains pending.
