# Evaluate generation-owned OTLP SDK logger reuse

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Inspected baseline: `e31cc36dc`; locked OpenTelemetry API/SDK 0.31.0.

## Evidence and possible benefit

HTTP terminal logging moves its serialized String through AccessLogger and the
key-only OtlpLogSender into OtlpLinkClient::send_with_meta. The client checks
health, loads the current Pipeline and calls log_sender::emit_record. Each emit
calls provider.logger("edgion-gateway") before constructing the record.

A logger stored in that Pipeline could reuse its instrumentation scope and
provider handle across records. The SDK currently constructs a scope from a
static borrowed name and empty attributes, then clones its provider Arc. Do not
claim a new scope-name heap allocation per record or measured throughput gain.
Caching adds one retained logger/provider handle per live pipeline generation.
SDK BatchLogProcessor::emit independently clones the record and instrumentation
scope into a boxed queue item; caching the logger does not remove that operation.

## Equivalence boundary

In locked SDK source, logs/logger_provider.rs::logger_with_scope checks the
provider shutdown flag and returns a logger backed by a no-op provider after
shutdown starts. logs/logger.rs::SdkLogger::emit visits its retained provider's
processors without that flag check. The standard batch processor's emit also
has no equivalent provider shutdown gate before its queue attempt.

Gateway client health is checked before acquiring the Pipeline snapshot. A send
that has passed that check and retained a pipeline may overlap shutdown, which
withdraws the pipeline and calls provider.shutdown_with_timeout from retirement.
The SDK sets its shutdown flag before shutting down processors. A cached logger
would bypass the current late logger-acquisition branch in such an interleaving.
This is source evidence of differing paths, not a reproduced live delivery race,
a guarantee of delivery loss or a claim that all post-shutdown emits export.
Even the current check is not an atomic send/retire fence.

Decide whether equivalence can be proved within existing ownership or whether
this changes an accepted lifecycle boundary. Do not introduce a global barrier,
durable-delivery promise or second provider owner to justify a small optimization.
Retain per-generation bounded cleanup and exporter release; old handles must not
keep a provider worker alive beyond its supported cancellation path.

## Acceptance

Use controlled send/shutdown interleavings to compare the actual production SDK
batch path, including a send paused after health/snapshot selection and a
provider paused during shutdown. Separate synthetic SDK branch observations from
reachable Gateway scheduling and external delivery evidence. Exercise healthy
sending, queue saturation, generation replacement, deletion, process shutdown
and cancellation of shutdown waiters. Prove exporter/channel release with a
retained logger handle; preserve current best-effort delivery limits.

Verify identical body, instrumentation scope, typed request trace ID without an
invented span, timestamps, severity, method and sensitive path rendering. Keep
SDK internal diagnostics disabled and preserve first-healthy selection without
send-error fallback. Compare CPU, allocation and generation-lifetime overhead
before adoption; do not infer savings from removing a method call alone.

Canonical constraints:
- skills/04-review/link-sys/regression-guards/otlp-provider-ownership-and-egress.md
- skills/04-review/link-sys/regression-guards/link-sys-hot-reload-reuse-and-retirement.md
- skills/04-review/link-sys/intentional-design/provider-owned-runtime-and-simple-removal.md

No runtime implementation or controlled concurrency probe was added by this task.
