---
name: process-exit-log-delivery-best-effort
description: Use when reviewing DataSender process-exit flushing, missing writer joins, global logger teardown, or proposals for a guaranteed shutdown drain.
knowledge_type: intentional-design
---

# Best-effort log delivery during Gateway process exit

## Conclusion and scope

On 2026-09-12, the user explicitly accepted possible loss of queued or in-flight
logs at Gateway process exit and declined a shared DataSender shutdown/drain
contract. Restarts can have deadlines; Edgion does not promise that all logs are
sent before exit. The user requested knowledge retention and deletion of
`data-sender-graceful-shutdown-contract` (campaign reference `D0011`).

This is an accepted reliability limitation, not a fixed defect or a disproved
finding. Do not report the absence of a process-wide sender drain, local-file
writer join, or guaranteed final OTLP flush again under these same assumptions.
First check the re-evaluation conditions below; this decision does not suppress
distinct delivery bugs or changes that prematurely stop logging during request
drain.

## Rationale and evidence

- [Gateway startup/shutdown](../../../01-architecture/02-gateway/01-startup-shutdown.md)
  and `edgion-gateway/src/cli/mod.rs`, `EdgionGatewayCli::run`: the auxiliary
  runtime remains active while Pingora runs. After `run_server` returns, the
  Gateway signals auxiliary services, allows five seconds, and joins the runtime
  thread. That window is an opportunity to send, not an acknowledgement that
  every log queue is empty. It is not a dedicated sender drain budget or a hard
  upper bound on the entire process shutdown.
- `edgion-gateway/src/link_sys/runtime/data_sender.rs`: `DataSender` exposes
  admission and health operations, without a shutdown receipt. `healthy()` does
  not establish delivery or queue completion; see the
  [health contract](healthy-semantics-intentional-non-flips.md).
- `observe/access_log/mod.rs` and `observe/logs/{tcp_log,tls_log,udp_log}.rs`
  (under `edgion-gateway/src/`) retain loggers in global `OnceLock`s.
  `AccessLogger.senders` owns the output adapters. LocalFile and OTLP own their
  sending machinery; ES/Kafka adapters resolve shared clients by resource key.
- `link_sys/providers/local_file/data_sender_impl.rs`: admission uses
  `LocalFileWriter.sender.try_send`; a detached file thread writes and flushes
  each batch. `observe/logs/ssl_log.rs` adds a Tokio forwarding queue. Neither
  path provides a process-exit completion acknowledgement.
- `link_sys/providers/otlp/log_sender.rs`: `OtlpLogSender.provider` is retained
  by the global logger. Its comment describing automatic flush via AccessLogger
  drop must not be read as an actual process-exit hook; these globals are not
  dropped by the shutdown sequence. The
  [OTLP integration evidence](../../testing/regression-guards/integration-suite-lifecycle-errors-must-fail.md#otlp-shutdown-delivery)
  explicitly distinguishes delivery during the grace period from provider drop.

## Alternatives and limits

The accepted benefit is a small, non-blocking sending path and unchanged restart
behavior without coordinating four providers, shared client ownership, forwarding
queues, and shutdown ordering. The cost is possible tail-log loss, no explicit
all-senders completion result, and no promise of remote acknowledgement or durable
file persistence. The frequency and volume of shutdown loss were not measured.

The user declined indefinite waiting. A common deadline with explicit draining,
writer joins, and completion reporting was also considered but judged insufficient
value for the required lifecycle changes. Early shutdown-triggered acceleration
of batching is not required. No new delivery guarantee, metric, runtime option,
or lifecycle framework is requested by this decision.

This acceptance concerns process exit only. It does not accept stale-generation
delivery or health publication after LinkSys replacement/removal; the separate
[LinkSys runtime apply lifecycle](../../../../tasks/todo/linksys-runtime-apply-lifecycle/linksys-runtime-apply-lifecycle.md)
remains open. Existing provider cleanup and cancellation contracts remain in
force. The four grandfathered plugin failure-policy shapes are unrelated to
sender shutdown; their count is not evidence against a lifecycle change.

## Verification and re-evaluation

Current source and shutdown call chains were inspected for this decision. No
runtime code, configuration, or user-facing behavior changed; no new delivery
test was added or run. Earlier OTLP integration results in the linked knowledge
page are historical and do not prove that all buffered logs survive exit.

Closure verification on 2026-09-12: independent read-only review returned LGTM;
`cargo fmt --all`, `cargo check --workspace --all-targets`, and
`cargo clippy --workspace --all-targets` passed (Clippy reported existing
warnings in unchanged Rust sources). The agent-doc, SSA-force, metrics-inventory,
and Pingora-source-policy guards passed. These checks validate the documentation
closure and workspace baseline, not a new runtime delivery guarantee.

Reconsider only when there is materially new evidence or an explicit requirement:

- Normal restarts repeatedly lose an operationally significant volume of logs.
- A product requirement introduces a flush acknowledgement, a dedicated drain
  budget, or stronger delivery guarantees.
- Shutdown ordering or runtime ownership changes so logging is stopped before
  the intended request-drain opportunity completes.
- Logger hot reload or independent in-process teardown introduces a lifecycle
  requirement that cannot rely on process exit.

Missing joins, global lifetime, finite waits, or theoretical tail loss alone are
the already-accepted case, not new evidence requiring another finding.

## Resolution references

- `data-sender-graceful-shutdown-contract` / `D0011`: accepted risk; improvement
  declined by the user on 2026-09-12; no code fix.
