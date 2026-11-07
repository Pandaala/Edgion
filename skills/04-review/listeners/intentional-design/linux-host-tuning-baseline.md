---
name: linux-host-tuning-baseline
description: Use when reviewing Linux gateway sysctl templates, FD limits, or TIME_WAIT tuning defaults.
knowledge_type: intentional-design
---

# Explicit Linux host tuning baseline

## Conclusion and scope

The operator-approved baseline offers manually installed `somaxconn=65535` and
`NOFILE=1048576` examples. It does not mutate the host at gateway startup and does
not set `tcp_tw_reuse` to any value, including `2`. Other network sysctls, process
limits, and allocator settings remain conditional tuning, not blanket defaults.
This is deployment guidance, not an active-connection admission guarantee.

## Rationale and evidence

The user explicitly accepted preserving `tcp_tw_reuse` after reviewing current
Linux semantics and Envoy/Traefik operational guidance. The original task's
unconditional `tcp_tw_reuse=1`, `LimitNPROC=512000`, short FIN timeout, enlarged
port ranges, and buffers were superseded by this scoped decision.

- `edgion-gateway/src/cli/pingora.rs::create_server_conf` resolves local YAML
  worker count, work stealing, and pool size (default 128); changes need restart.
- The locked Pingora revision `edgion_v0.9.1@e5f4287be713d01ffd444544251bf75c9d00062a`
  (re-verified by source inspection on 2026-11-06; originally recorded at
  `518f7274`) uses `LISTENER_BACKLOG=65535` in `pingora-core/src/listeners/l4.rs`.
- Its `ConnectorOptions::from_server_conf` scales pool capacity by worker count;
  this is not one global gateway-wide active/idle connection cap.
- The default tikv-jemalloc build uses the `_rjem_` prefix, hence
  `_RJEM_MALLOC_CONF`; profiling is not enabled by the ordinary feature graph.
- [Linux TCP documentation](https://docs.kernel.org/networking/ip-sysctl.html)
  distinguishes global and loopback-only reuse and warns against unqualified
  changes. `tcp_fin_timeout` concerns orphaned FIN_WAIT_2, not TIME_WAIT.

See the [operator guide](../../../../docs/en/ops-guide/kernel-tuning.md) for the
complete external references and [assets](../../../../cicd/deploy/linux-tuning/README.md).

## Alternatives and limits

A high FD allowance avoids a small inherited ceiling but does not provision RAM,
ports, conntrack, or CPU. Larger backlog can increase queue latency and memory.
CPU-only alerting and a universal connection capacity inferred from NOFILE are
rejected. Global TIME_WAIT reuse can be considered separately when port pressure
is measured and real peer behavior is tested. Absence of a recommendation in the
consulted Envoy/Traefik pages is not proof that they never discuss the parameter.

PAM, systemd, and container limits have different inheritance paths. Pod network
namespace values must be verified independently of node configuration. Full
service/package integration remains outside this manual tuning baseline; the
examples in the operator guide do not provide a complete service/package adapter,
and adding one requires separately approved work.

## Verification and re-evaluation

Source and locked dependency inspection support the values above. The artifacts
are documentation and inert templates; no Linux host settings were applied in
the macOS development environment. Linux deployment/load validation remains an
operator prerequisite, not a claimed test result. Re-evaluate after Pingora
backlog/pool changes, allocator feature/prefix changes, kernel policy changes, or
workload measurements demonstrating a different bottleneck.

Implementation verification (2026-09-20): independent read-only review passed;
`cargo fmt --all`, `cargo check --workspace --all-targets`, and
`cargo clippy --workspace --all-targets` completed successfully. Clippy reported
existing `redundant_async_block` and `err_expect` warnings in unchanged test code.
Agent-doc, SSA force, metrics inventory, and Pingora source-policy guards passed.
Additional static checks passed for bilingual code/heading parity, shell example
syntax, local links, the sysctl active-key allowlist, and systemd/PAM limit values.
No full test suite, Linux deployment test, or load benchmark was run.

## Resolution references

- `linux-kernel-tuning-and-sysctl-profiles` (created 2026-09-13): documentation-only
  implementation with explicitly accepted conservative host policy; original
  blanket tuning requirements superseded by the user's scoped decision.
