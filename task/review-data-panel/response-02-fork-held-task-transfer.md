# Decide fork response-head release task transfer

Status: deferred for user decision. Owner: the Edgion Pingora fork, not a
Gateway-local buffer. Inspected against Edgion `8990c789e` and locked fork
`1aaa45cc70430bc782990bc20ddee3fef36d6b99`.

## Evidence and candidate

In `pingora-proxy/src/response_head_barrier.rs`,
`ResponseHeadBarrier::capture_or_release` handles a successful release with
`tasks.split_off(start)`, extends `held.tasks` with that temporary suffix Vec,
then extends `tasks` with the combined held/current tasks. The observable order
is untouched prefix, previously held tasks, then current suffix. These are
ownership moves; this is not a body-payload clone finding.

The same function's continuing-Hold branch already transfers the suffix with
`held.tasks.extend(tasks.drain(start..))`. Investigate using that transfer in
the successful release branch to avoid the temporary suffix allocation when
the suffix is nonempty. An empty suffix has no corresponding allocation saving.
Destination Vec growth and retained capacity remain separate costs; there is
no measured throughput, latency or peak-memory result here.

The source-error scan, deadline check, candidate accounting, checked combined
usage and limit checks all precede the successful release branch. Keep their
ordering and failure behavior. The separate failed-task `split_off` path also
owns error selection and remaining-task destruction; it is outside this narrow
candidate and must not be mechanically changed with the release branch.

## Decision and acceptance

Decide whether this dependency-owned saving warrants a fork patch and coordinated
dependency adoption. Do not edit the Cargo checkout as an Edgion implementation,
add a local buffering workaround, or change Cargo.lock without reviewing the fork
change. Preserve the single-revision family required by the Pingora source policy.

- Compare old/new release results for zero and nonzero `start`, empty and
  nonempty held/current ranges, header/body/trailer order and EOS flags. Preserve
  the untouched prefix, PrepareFrom index and exactly-once ownership transfer.
- Preserve source-error precedence, absolute deadline behavior, all independent
  limits, usage publication, release latch disarming, cancellation cleanup and
  final-head writer ownership. No response BodyStorage, spool, broader retention,
  new observability or failure-policy change is authorized.
- Run the fork's barrier and response-pipeline regressions, then relevant H1/H2
  pump and Gateway owner tests. Existing
  `release_appends_current_after_all_held_tasks` covers a nonempty suffix at
  `start == 0`; it is not proof of the whole proposed matrix. Include failure,
  timeout and cancellation cases; inspect ownership destruction on early exits.
- Measure allocation calls/requested bytes separately from destination capacity
  and peak retained storage, varying held/current lengths and existing capacities.
  Do not infer application throughput from an isolated allocation count.
- Adopt through IMPL with an exact fork/repository change scope and run
  `cicd/checks/check_pingora_source_policy.py` after dependency updates.

This record changes no runtime code. The cited fork tests were inspected, not
executed for this finding. The separate Guardrail template decision remains in
[guardrail-01](guardrail-01-held-replacement-header-storage.md). Full callback
multiplicity, retry and transport interleavings remain open in
[CALLBACKS](CALLBACKS.md).
