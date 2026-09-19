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
finding. Do not report the absence of a shared cross-provider sender drain,
local-file writer join, or guaranteed remote acknowledgement and lossless tail
delivery again under these same assumptions. OTLP now has a bounded
process-shutdown retirement path; its absence would be a distinct regression,
not this accepted case. First check the re-evaluation conditions below; this
decision does not suppress distinct delivery bugs or changes that prematurely
stop logging during request drain.

## Rationale and evidence

- [Gateway startup/shutdown](../../../01-architecture/02-gateway/01-startup-shutdown.md)
  and `edgion-gateway/src/cli/mod.rs`, `EdgionGatewayCli::run`: the auxiliary
  runtime remains active while Pingora runs. After `run_server` returns, the
  Gateway signals auxiliary services, drains UDP producers first, calls
  `shutdown_otlp_clients`, and joins the runtime thread. The OTLP call cancels
  runners and joins generation cleanup and bounded SDK shutdown while Tokio IO
  is still available. Completion means the local retirement attempt finished;
  it is not a collector acknowledgement or proof that every tail record arrived.
  `OtlpStartupGuard` invokes the same shutdown path when startup fails after
  conf-sync can publish OTLP generations.
- `edgion-gateway/src/link_sys/runtime/data_sender.rs`: `DataSender` exposes
  admission and health operations, without a shutdown receipt. `healthy()` does
  not establish delivery or queue completion; see the
  [health contract](healthy-semantics-intentional-non-flips.md).
- `observe/access_log/mod.rs` and `observe/logs/{tcp_log,tls_log,udp_log}.rs`
  (under `edgion-gateway/src/`) retain loggers in global `OnceLock`s.
  `AccessLogger.senders` owns the output adapters. LocalFile owns its sending
  machinery; OTLP, ES, and Kafka adapters resolve shared clients by resource key.
- `link_sys/providers/local_file/data_sender_impl.rs`: admission uses
  `LocalFileWriter.sender.try_send`; a detached file thread writes and flushes
  each batch. `observe/logs/ssl_log.rs` adds a Tokio forwarding queue. Neither
  path provides a process-exit completion acknowledgement.
- `link_sys/providers/otlp/log_sender.rs`, `link_sys/providers/otlp/client.rs`,
  and `link_sys/runtime/{runner,store}.rs`: `OtlpLogSender` retains only the
  LinkSys key. `PersistentRunner<OtlpApply>` owns published client generations,
  and each `OtlpLinkClient` generation owns its SDK pipeline and sending state.
  `shutdown_otlp_clients` deletes and joins the runners; client retirement swaps
  out the pipeline and invokes the SDK's bounded shutdown before releasing the
  exporter. The
  [OTLP integration evidence](../../testing/regression-guards/integration-suite-lifecycle-errors-must-fail.md#otlp-shutdown-delivery)
  remains evidence of attempted shutdown delivery, not remote acknowledgement.

## Alternatives and limits

The accepted benefit is a small, non-blocking sending path and unchanged restart
behavior without a cross-provider DataSender drain contract coordinating four
providers, forwarding queues, local writers, and shutdown ordering. OTLP's
provider-specific bounded retirement narrows one part of that gap. The remaining
cost is possible tail-log loss, no all-senders completion result, and no promise
of remote acknowledgement or durable file persistence. The frequency and volume
of shutdown loss were not measured.

The user declined indefinite waiting. A common deadline with explicit draining,
writer joins, and completion reporting was also considered but judged insufficient
value for the required cross-provider lifecycle changes. The existing OTLP-specific
shutdown is not such a guarantee: it attempts an SDK flush and retirement within
bounds, but does not receive an end-to-end delivery acknowledgement. Early
shutdown-triggered acceleration of batching is not required. No new delivery
guarantee, metric, runtime option, or shared lifecycle framework is requested by
this decision.

This acceptance concerns process exit only. It does not accept stale-generation
delivery or health publication after LinkSys replacement/removal; those runtime
fences are recorded in
[hot reload reuse and retirement](../regression-guards/link-sys-hot-reload-reuse-and-retirement.md)
and
[Kafka producer generation shutdown](../regression-guards/kafka-producer-generation-shutdown.md).
Existing provider cleanup and cancellation contracts remain in force. The four grandfathered plugin failure-policy shapes are unrelated to
sender shutdown; their count is not evidence against a lifecycle change.

## Verification and re-evaluation

Current source and shutdown call chains were inspected for this decision. No
runtime code, configuration, or user-facing behavior changed; no new delivery
test was added or run. Earlier OTLP integration results in the linked knowledge
page are historical and do not prove that all buffered logs survive exit. Current
source inspection confirms the key-only sender, generation-owned pipelines,
normal-shutdown UDP-before-OTLP ordering, and startup-error guard described above.

Closure verification on 2026-09-12: independent read-only review returned LGTM;
`cargo fmt --all`, `cargo check --workspace --all-targets`, and
`cargo clippy --workspace --all-targets` passed (Clippy reported existing
warnings in unchanged Rust sources). The agent-doc, SSA-force, metrics-inventory,
and Pingora-source-policy guards passed. These checks validate the documentation
closure and workspace baseline, not a new runtime delivery guarantee.

Reconsider only when there is materially new evidence or an explicit requirement:

- Normal restarts repeatedly lose an operationally significant volume of logs.
- A product requirement introduces remote flush acknowledgement, lossless tail
  delivery, or stronger cross-provider guarantees.
- Shutdown ordering or runtime ownership changes so logging is stopped before
  the intended request-drain opportunity completes.
- Logger hot reload or independent in-process teardown introduces a lifecycle
  requirement that cannot rely on process exit.

Missing local-file joins, global logger lifetime, finite waits, or theoretical
tail loss alone are the already-accepted case, not new evidence requiring another
finding. Removal or breakage of the existing bounded OTLP runner/client retirement
is a separate lifecycle regression and remains reportable.

## Resolution references

- `data-sender-graceful-shutdown-contract` / `D0011`: accepted risk; improvement
  declined by the user on 2026-09-12; no code fix.
