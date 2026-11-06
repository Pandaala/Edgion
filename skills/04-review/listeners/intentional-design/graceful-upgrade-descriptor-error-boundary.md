---
name: graceful-upgrade-descriptor-error-boundary
description: Review Edgion's Pingora upgrade reachability and the accepted residual descriptor cleanup limitation on malformed handoff.
knowledge_type: intentional-design
---

# Graceful upgrade descriptor error boundary

## Conclusion and scope

On 2026-10-05 the user accepted closing P090-007 without a production change
after distinguishing the already-imported ownership fixes from the remaining
malformed-handoff cleanup defect. The latter is accepted within the current
Edgion product boundary, not fixed or disproved. This decision does not certify
complete partial/error handoff cleanup or successful Linux upgrade acceptance.

Edgion's lockfile selects Pingora
`edgion_v0.9.1@e5f4287be713d01ffd444544251bf75c9d00062a`. That revision owns the accepted
transfer connection with `OwnedFd`, receives listener descriptors with
`MSG_CMSG_CLOEXEC`, and includes socket-pair EOF tests for unclaimed descriptor
cleanup and claimed descriptor preservation (`closes_only_unclaimed_fds`,
`empty_keep_set_closes_everything`). No further import of those fixes is needed
for this consumer revision. This and the residual-defect analysis below were
re-verified by source inspection at that revision on 2026-11-06 after the fork
was rebased onto upstream 0.9.1; `pingora-core/src/server/` (including
`transfer_fd/mod.rs` and `bootstrap_services.rs`) is unchanged from
`0b0d8bba9609655bfd7db0fe0add6818bc0ee937` (the earlier `edgion_0.9.0_v1`
revision; the immediate predecessor lock was `1aaa45cc`), on which the 2026-10-05
decision was originally made.

## Rationale and evidence

The [Gateway startup code](../../../../edgion-gateway/src/cli/pingora.rs)
sets `ServerConf.daemon` to false and calls
`Server::new_with_opt_and_conf(None, server_conf)`. In the locked fork,
`Bootstrap::new` consequently sets `upgrade` to false, so ordinary Gateway
startup does not receive inherited descriptors. This is stronger evidence than
`daemon: false` alone: `PolicyShutdownSignalWatch` still delegates external
signals, and SIGQUIT can reach Pingora's descriptor **sending** branch.

The locked fork's `pingora-core/src/server/transfer_fd/mod.rs` contains the
decisive ownership boundaries:

- `get_fds_from` wraps the accepted transfer connection in `OwnedFd` and uses
  `MSG_CMSG_CLOEXEC`. The temporary connection is released on normal return,
  error return, or panic unwinding.
- `Fds::get_from_sock` receives a `Vec<RawFd>` before parsing bind addresses.
  Invalid UTF-8 returns an error without closing those descriptors;
  `Fds::deserialize` panics if address and descriptor counts differ. Neither
  the vector of descriptor numbers nor `Fds` provides automatic close.
- `Bootstrap::bootstrap` exits on a returned load error, letting the kernel
  reclaim the process's descriptors. A library caller that remains alive after
  the error does not have that cleanup guarantee.
- `close_unclaimed` closes and removes addresses outside the keep set. A
  successful bootstrap mutates the existing shared `listen_fds` table, so
  services retain the same shared table. These successful-path properties do
  not prove malformed-handoff cleanup.

Current listener construction still obtains its PROXY trust and TLS setup from
the configured listener. Descriptor handoff does not redefine those policies or
the direct transport peer identity. The consumer startup lifecycle remains in
the [startup contract](../../../01-architecture/02-gateway/01-startup-shutdown.md).

## Alternatives and accepted cost

The user chose closure after the residual defect and product reachability were
explained. This avoids a local ownership workaround or a new binary-upgrade
feature solely to complete an adoption task. The accepted cost is incomplete
error cleanup in the underlying public reception API, plus absence of fresh
Linux descriptor-transfer and real listener handoff acceptance in this run.

The acceptance is limited to the current consumer configuration. It is not
permission to use the reception API in a surviving process without revisiting
its ownership contract, nor evidence that SIGQUIT is unreachable. General fork
error-path hardening remains subject to its upstream-defect maintenance policy.
No production code, dependency, configuration, or user-facing API changes here.

## Verification and re-evaluation

The 2026-10-05 source inspection used the then-locked fork `0b0d8bba`, not
the older local maintenance checkout; the 2026-11-06 re-inspection used
`e5f4287` revision objects, not a working tree. Independent read-only review confirmed both the imported fixes and
the residual error-path defect. The existing Linux transfer/CLOEXEC tests and
Unix socket-pair tests were inspected, not executed for this decision. No fresh
runtime, subprocess, or normal listener-service acceptance is claimed.

Fresh documentation-closure checks on 2026-10-05 passed:

- `cargo fmt --all` and `cargo fmt --all -- --check`.
- `cargo check --workspace --all-targets` and
  `cargo clippy --workspace --all-targets`.
- `python3 cicd/checks/validate_agent_docs.py`.
- `python3 cicd/checks/check_ssa_force.py`.
- `python3 cicd/checks/check_metrics_inventory.py`.
- `python3 cicd/checks/check_pingora_source_policy.py`.
- `python3 cicd/checks/generate_gateway_api_transform_contract.py --check`.
- `python3 cicd/checks/check_unit_test_layout.py`.

The first Clippy attempt encountered an incomplete concurrent retry-test enum
match; the retry implementation subsequently supplied the missing branch and
workspace check and Clippy passed on retry. A later format check found wrapping
differences in that concurrent test file; formatting was applied and rechecked.
These results include the shared worktree's concurrent changes, not a review
of that separate retry implementation. No full workspace tests were run.

Independent closure review returned LGTM for the accepted disposition, current
source evidence, knowledge classification, and updated inbound references.

Re-evaluate when Edgion enables the receiving upgrade option, a caller survives
a malformed reception, the locked fork changes, a normal upstream fix is
available, or actual descriptor leakage appears on a supported Gateway path.
Any future upgrade feature needs explicit ownership and Linux process tests;
this closure does not satisfy that future feature's acceptance gates.

## Resolution references

- P090-007: upstream ownership fixes already imported; residual malformed
  descriptor-handoff cleanup accepted without a production fix on 2026-10-05.
