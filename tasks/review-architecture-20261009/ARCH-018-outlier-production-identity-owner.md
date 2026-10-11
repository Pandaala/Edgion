---
id: ARCH-018
status: confirmed
kind: architecture-debt
priority: P2
primary_partition: PART-255
independent_verification: confirmed
---

# Retire the unused socket-hash outlier execution owner

## Current code and concrete verification cost

backends/outlier_detection/store.rs retains two ServiceCbState containers:
socket-hash endpoints and EndpointRuntimeIdentity identity_endpoints. The former's
is_ejected, record_failure, record_success and remove_backend APIs remain
production-compiled, with their own percent arbitration, recovery and drain logic.
Workspace caller searches, including conditional sources and the harness, find
HTTP/TCP/TLS production completion paths using coordinator-captured tokens and
record_*_for_backend_with_token. Discovery removes identity state; policy configures
service state; diagnostics count services. Socket-hash execution callers are tests.

The regression percent_arbitration_runs_only_for_ejection_candidates exercises
the socket-hash method. Its percent_arbitrations counter increments only in that
method, not the actual identity arbitration branch. A regression moving production
arbitration before candidate admission could leave this specifically named test
green. Most store-level percent/concurrency/recovery tests similarly validate the
old wrapper. Existing coordinator/selector tests cover real identity lifecycle but
do not make obsolete wrapper assertions authoritative for production arbitration.

A superseded execution owner thus survives through its tests. Changing percent
limits, cooldown or gauge settlement requires inspecting two wrappers while some
regressions exercise the unused one. The current identity algorithm correctly
fences policy/membership and admits candidates first; no current over-ejection,
gauge leak or stale-completion acceptance is reported.

## Bounded improvement and alternatives

Repeat the caller audit during implementation. Migrate store-level arbitration,
cooldown/backoff/recovery and replacement assertions to isolated coordinator
fixtures using the actual identity/token methods. Remove the socket-hash container,
unused execution/removal APIs and hash-only drain paths. Relocate arbitration
instrumentation to the production branch. A test-only fixture may delegate to the
real owner; it must not maintain another algorithm.

Keep PassiveEjectionState and its kernel tests. expire_legacy_ejection and
legacy_ejection_remaining remain used by the identity/coordinator cooldown protocol;
their names do not justify removal. AI half-open and credential outlier behavior
remain unchanged. Removing regressions would lose protection. Historical zero-total
cases need reconciliation with coordinator-owned count, not mechanical deletion
or caller-supplied production counts retained only for test convenience.

## Invariants, risks and future checks

Preserve policy identity fencing, candidate revalidation under arbitration,
retirement, exactly-once gauge settlement, cooldown/backoff history and independent
failure classification. Existing accepted endpoint-recovery/service-arbitration
locks, scans and metric-label allocation are outside scope. External consumers
were not inventoried; pre-release project rules require no compatibility shim.

Future production-path regressions should prove sub-threshold failures do not
arbitrate, concurrent candidates respect caps, cooldown/success settle once,
policy/membership changes reject late completions, retirement cannot resurrect
state, and shared AI behavior remains intact. Preserve HTTP/TCP/TLS integrations
and test identities where feasible.

## Original authority and review limits

The analyst fully read all four outlier primaries, completion and retirement
callers, wrapper/state/coordinator tests and original runtime-snapshot,
candidate-only arbitration, zero-total, success-gauge, config-race, constructor
and integration-cooldown records. Active-health's explicit hash compatibility
decision concerns HealthStatusStore, not this twin. Backend architecture retains
historical socket-hash descriptions alongside the newer identity contract; update
that owning section if implementation removes this owner. No decision requiring
this outlier twin was found. Static review only; independent verification pending.
