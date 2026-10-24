# UDP expiry candidate ownership

Status: deferred for user decision. No runtime design selected.
Source baseline: `5ba36c4f9`.

## Evidence and scenario

`cleanup_expired_sessions` collects expired peer addresses and cloned session Arcs
into a Vec using one collection Instant. It then rechecks and retires candidates
sequentially, drops the current session owner and awaits its terminal log. The
iterator still owns the unprocessed tail across that await.

For example, candidates A and B are collected. Cleanup retires A and waits for
its logger. B's receiver independently retires B and releases its own session
owner. The unprocessed candidate Arc can still keep B's session, permit and
socket Arc alive until cleanup reaches B. The current-session drop before logging
already avoids this for A; this task concerns the remaining tail.

The list is bounded by collected sessions. This is conditional lifetime overlap,
not an unbounded leak, per-packet copy, or new packet-buffer allocation. Socket
closure also depends on independent socket owners.

## Decision alternatives

- Retain strong candidates if simplicity and lower bookkeeping cost outweigh
  lifetime overlap under expected logging and session-churn workloads.
- Evaluate Weak candidates, upgrading only when processing each candidate and
  retaining the existing canonical retirement checks on the upgraded Arc.

No alternative is selected. Weak storage still retains the backing Arc allocation,
including storage for a value whose fields have been dropped. It does not make
the entire allocation disappear early. Upgrade adds atomic work and can fail;
earlier release of permits also changes when capacity becomes available for new
sessions. Compare these effects explicitly rather than claiming a free saving.

Do not replace candidates with peer keys alone: an old candidate must never
retire a replacement generation under the same address. Do not batch all
retirements before logging as part of this candidate; that changes the timing of
activity rechecks and sequential retirement. UDP-01 wakeups, UDP-02 receiver
ownership and the recently shortened current sender/shutdown owners are separate.

## Correctness and acceptance gates

Preserve the collection Instant, timeout predicate, sequential processing,
`Arc::ptr_eq` identity guard and lifecycle-locked activity recheck. A current map
entry retains its own strong owner; a failed upgrade must not become permission
to act on a newer entry. Successful send activity still wins over expiration.
Only the path that removes a generation submits its terminal log; neither weak
upgrade failure nor stale identity creates another log.

Real in-flight sender/receiver owners keep their session permit. Releasing a
candidate's ownership must not bypass that rule. Preserve cancellation, task
tracking, normal shutdown joins, failure cleanup, source pinning and counters.
Introduce no additional registry, polling task, tracing or metrics.

Before implementation, freeze an allow-list and obtain independent IMPL plan and
final review. Controlled tests should hold A's logger pending while B is retired
by its receiver, then inspect permits, session destruction and remaining backing
allocation separately. Cover same-peer recreation, activity refresh after
collection, live/failed upgrades, remaining sender owners, cancellation and
aborted cleanup. Existing whole-task completion tests do not prove this overlap.

Measure strong/weak bookkeeping, cleanup latency and live resource overlap for
ordinary and blocked logging with session churn. No throughput, allocation-count
or RSS improvement has been measured for this proposal.

## Sources and current verification

- `edgion-gateway/src/routes/udp/edgion_udp.rs`: cleanup_expired_sessions,
  retire_session and receiver retirement.
- [Generation retirement](../../skills/04-review/listeners/regression-guards/udp-session-generation-retirement.md).
- [Shutdown ownership](../../skills/04-review/listeners/regression-guards/udp-process-shutdown.md).

Fresh unchanged-source checks passed 59 UDP route/session and four UDP log tests,
formatting and workspace all-target check/Clippy with existing warnings. These
validate baseline behavior, not Weak adoption. No prototype, blocked-logger
experiment, benchmark, full suite or datagram wire test ran.
