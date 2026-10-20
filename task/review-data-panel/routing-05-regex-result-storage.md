# Evaluate multi-pattern regex route result storage

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `d75c4f1c8`.

## Evidence and decision

`RegexRoutesEngine::match_route` in
`edgion-gateway/src/routes/http/match_engine/regex_routes_engine.rs` enumerates
RegexSet matches in ascending index order for multi-pattern sets. Build-time
sorting makes that the required candidate priority. Results live across the
synchronous deep-match loop and are dropped when matching returns.

The locked regex 1.13.1 `regexset/string.rs::matches_at` creates a PatternSet.
In regex-automata 0.4.18, `util/search.rs::PatternSet::new` allocates boxed boolean
storage sized to the pattern count. The hidden `matches_read_at` API also
constructs that PatternSet before copying into caller storage; passing a reusable
boolean buffer to it does not remove this allocation.

The singleton case already uses the same set's boolean query and needs no result
collection. This task concerns multiple patterns only. Reusing a lower-level
matcher result set or scratch requires a design for ownership, concurrent and
reentrant borrowing, retained capacity, and publication generations. No owner
(request context, thread-local slot, pool or shared lock), dependency change or
replacement regex engine is approved here.

Compare concrete designs with current ephemeral allocation before deciding.
Retaining maximum capacity can increase idle memory; sharing scratch can add
contention or reentrancy hazards. A lower-level API can alter engine defaults
and cold failure behavior. Allocation evidence alone does not prove throughput
benefit. Keeping the current implementation is a valid outcome.

## Acceptance for later investigation and implementation

- Preserve ascending positional IDs against the matching immutable route
  generation. Do not replace all matches with an arbitrary first regex hit:
  a higher-priority path match may fail its ancestor, listener, method, header
  or query conditions, allowing the next candidate.
- Preserve radix-before-regex within each hostname group, ordered named groups
  before eligible hostless fallback, and current error propagation. Only the
  appropriate no-match result advances to another candidate or group.
- Preserve original pattern syntax, UTF-8 semantics, empty matches, anchors,
  flags, compilation defaults and size limits. Retain cold set-compilation
  failure's existing linear matching fallback. Use one canonical implementation
  rather than maintaining competing regex semantics.
- Preserve singleton behavior and successful route/Gateway Arc ownership.
  The returned owners outlive the lookup snapshot; result-storage work does not
  authorize replacing them with references.
- Define scratch capacity growth, reset, bounds, concurrent/reentrant access,
  old/new snapshot isolation and reclamation. Do not introduce request-time
  compilation, a path-result cache, new logging or metrics to support the change.
- Measure multi-pattern counts and lengths, hit density, misses, early and late
  deep-match winners, repeated hostname groups, concurrency and configuration
  replacement. Report matching CPU, allocation count, requested/live/retained
  bytes and cold build cost with machine, profile, input and baseline details.
  Distinguish allocator counters from RSS and averages from tail latency.
- Test real session selection with ancestor/method/header rejection, priority,
  error propagation, old/new generations and capacity reset. Start from the
  existing regex engine and HTTP conf-handler tests, including the
  [ancestor-boundary guard](../../skills/04-review/routing/regression-guards/delegation-root-prefix-inherits-scope.md).
  Obtain independent implementation-plan and final review and run project gates.

No candidate implementation or comparative benchmark has been run in this audit.
