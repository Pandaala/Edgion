---
name: local-shared-topology-single-owner-takeover
description: Use when reviewing concurrent local integration runs, fixed shared-topology ports, or broad cleanup of local test binaries.
knowledge_type: intentional-design
---

# Local shared integration topology allows one runner and global cleanup

## Conclusion and scope

The canonical local integration topology permits one active owner per host
environment. It uses fixed Controller, Gateway, test-server, and fixture ports,
so independent worktrees are not supported as concurrent owners of that topology.

`start_all_with_conf.sh` intentionally terminates matching known local integration processes
before starting a new shared topology. `run_integration.sh` delegates ordinary
teardown to `kill_all.sh`, which likewise stops matching shared-topology binaries.
Overlapping runs are unsupported: starting or cleaning either run may clear the
shared environment instead of waiting for, or reporting a conflict with, the
other worktree run.

This decision covers only the local shared integration entry point and the known
Controller, Gateway, test-server, and managed fixture processes it uses. It does
not authorize broad process-name cleanup in production,
Kubernetes integration, isolated integration scenarios, or standalone jobs that
own separate ports and work directories.

## Rationale and evidence

The topology's fixed host ports are a single shared resource. Keeping another
worktree's processes alive cannot make the two runs independent: their Gateway
listeners and other fixed endpoints would conflict even if cleanup were narrowed
to run-recorded PIDs. The project therefore chooses a single-run invariant with
global test-process cleanup over a cross-worktree lock, waiting protocol, or
dynamic port allocation.

The current call chain is
`run_integration.sh` -> `start_all_with_conf.sh::kill_all_processes` for startup
and `run_integration.sh::cleanup` -> `kill_all.sh` for teardown. The startup
patterns cover `edgion-controller`, `edgion-gateway`, and `test_server` binaries;
the teardown path gives the Gateway a longer grace period under coverage before
escalating. Startup subsequently checks the fixed ports and fails closed if they
remain occupied.

On 2026-09-20, the project owner explicitly confirmed that one environment may
run only one shared integration topology and that clearing all matching topology
processes is acceptable. The previously proposed cross-worktree lifecycle
isolation would preserve a concurrency mode the topology does not support.

## Alternatives and limits

A host-wide ownership lock would turn takeover into wait-or-busy behavior but
would not enable parallel execution. Run-qualified ports would require a larger
redesign of configuration, fixtures, scenario assumptions, and clients. Both
alternatives add machinery without changing the current single-owner product
decision.

The accepted cost is that starting or cleaning the canonical topology in one
worktree can interrupt an in-progress canonical run in another worktree. Such an
interruption is test-environment contention, not evidence of a product or relay
defect. Operators who need concurrent execution must use distinct host
environments or only scenarios explicitly documented as isolated.

This decision does not relax lifecycle failure reporting: startup, readiness,
port, or teardown failures must still be surfaced by the runner. It also does not
turn arbitrary name-based process cleanup into a general shell-script pattern;
the exception is bounded to these known local integration processes and this
fixed-port topology.

## Verification and re-evaluation

The disposition is based on the current script call chain and the explicit
single-owner decision. No runtime code or test behavior changed, and no live
integration run was required to record the tradeoff.

Re-evaluate if the canonical local topology gains run-qualified ports, concurrent
worktree support becomes a requirement, cleanup patterns expand beyond the known
test binaries, or the scripts can target non-test processes in a supported usage.

## Resolution references

- `isolate-shared-integration-process-lifecycle` — accepted single-owner takeover
  on 2026-09-20; the proposed cross-worktree isolation was declined.
