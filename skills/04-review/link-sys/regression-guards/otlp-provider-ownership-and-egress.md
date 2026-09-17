---
name: otlp-provider-ownership-and-egress
description: OTLP LinkSys identity, dial policy, shared generations and SDK teardown boundaries.
knowledge_type: regression-guards
finding_id: D0023
---

# OTLP provider ownership and safe transport

## Scope and invariant

OTLP/gRPC log export is owned by a dedicated LinkSys client. StringOutput retains
only a validated namespace/name reference; the key, never endpoint or resolved
address, is the diagnostic and metric identity. Preserve the original LogMeta
projection, first-healthy selection and TCP-level health interpretation.

The former inline sender duplicated providers/probes and passed raw endpoints to
SDK channels, TCP probes, metrics and logs. It bypassed canonical egress and made
URI secrets observable. A key-only sender cannot recreate a transport or probe.

## Mechanisms to verify together

- Resource origin grammar rejects unsupported/sensitive URI components before
  formatting errors. Process LinkSysRef deserialization bounds namespace/name;
  programmatic sender construction validates again.
- Controller AuthSpec/SecretSlot and shared TLS loading clear old material and
  dependencies, visit all refs even after failure, and publish complete material.
  Resolved secrets are schema-excluded and admin-redacted, but serialized for sync.
- Native tonic014 channel retains hostname authority/SNI while each connector and
  probe dial calls canonical EgressResolver on exact addresses. No hostname fallback,
  environment proxy or redirect path exists. Custom CA replaces public roots.
- An interceptor overwrites SDK environment metadata with the resolved bounded
  Secret-only map. Authorization metadata is marked sensitive. SDK internal logs
  are disabled and shared mandatory filtering suppresses tonic::transport/rustls/h2
  diagnostics even under RUST_LOG=trace.
- The shared per-key persistent runner publishes an immutable OTLP generation
  only after its bounded empty Export succeeds and desired fingerprint is still
  current. Invalid, unresolved, or unreachable replacements keep old service.
  Candidate initialization and active health probing remain generation-local.
- Endpoint timeout covers headers only. The narrow SDK exporter decorator must
  bound the whole future and poll with captured runtime context on the SDK std
  thread. Retirement cancellation terminates stalled-body futures and queued exports.
- SDK shutdown timeout is not worker termination. Retain cleanup before any await,
  keep its cancellation timer owned by that task, join the SDK thread, and signal
  exporter/channel release only after dropping the inner exporter. Limit retirement
  to one generation; cancelled/concurrent shutdown waiters cannot orphan cleanup.
- Full/partial desired updates execute in the same ordered generic dispatch task
  as Redis. A later unrelated delta cannot suppress an earlier OTLP key. Type
  changes, removals, and process shutdown use the persistent-runner lifecycle.
  OTLP store stats/counts report runner keys and participate in residue detection.

## Source-order and reference-identity regressions

OTLP-REVIEW-001 exposed a gap outside the former OTLP owner: a resource changed from a
foreign provider to OTLP could keep its old typed client if an unrelated later
delta reached the global Webhook sequence fence first. All three production
source callbacks now reserve generic async admission through `spawn_ordered_dispatch`
inside the source-event lock. Each task awaits its predecessor before entering
the existing Webhook mutation guard. This orders old full/partial publications,
type-change cleanup, and subsequent recreation without modifying typed getters
or rebuilding the full resource set for a delta. The private tail retains one
handle; pending tasks still scale with outstanding source events, as before.
Predecessor failure releases successors with a fixed diagnostic. No public task
abort API is exposed, and no synchronous lock is held across an await.

OTLP-REVIEW-002 exposed a post-validation resolution gap: lenient admission kept
parsing an invalid `group`/`kind`, and the generic lookup selected a same-name core
Secret. Admission and OTLP resolution now share `validate_otlp_secret_ref`.
Invalid auth, CA, and mTLS references neither resolve nor register dependencies;
previous material and edges are cleared. Valid sibling references are still
visited, and TLS material is published only as a complete generation. The
`OtlpTlsSpec` adapter uses the shared TLS loader; other providers retain their
existing policy. Invalid references use the existing `InvalidKind` condition
reason with fixed field identities, without echoing malformed input. The Gateway
still independently validates before activation; the original defect was wrong
Secret resolution/transport, not a ReferenceGrant bypass or credential egress.

Regression tests in `runtime/store.rs` force successor polling before gated
generic full/partial OTLP dispatch, check all foreign typed maps and the callable
Webhook registry, and verify later same-key recreation and predecessor panic. Provider
constructors remain uninitialized, so these tests open no network connections.
Controller handler tests cover invalid group/kind for auth, CA, and mTLS with
same-name Secrets and permissive grants, stale-material cleanup, valid sibling
dependencies, transport serialization, and recovery after correction. The isolated
OTLP driver adds live invalid-reference and registered-Redis-to-OTLP phases.
These cases do not claim to execute every older in-flight full-snapshot
interleaving or prove durable log delivery from unit tests.

Verification for these two corrections on 2026-09-12: runtime store 24, Controller
LinkSys handler 39, OTLP resource 7, and startup-drain 1 targeted tests passed.
The isolated dynamic run at
`tmp/integration_testing/otlp_testing_20260912_162325_83323/report.log` passed,
including both invalid-identity phases, registered Redis removal on OTLP restore,
all five identified shutdown records, and owned process/container cleanup.
The final Controller diagnostic wording adjustment followed that binary build;
workspace check and Clippy were rerun afterward. Formatting, agent-doc, SSA,
metrics inventory, Pingora source, English-only and gateway no-tracing guards
passed. Clippy still reports pre-existing workspace warnings. Independent final
code and knowledge review returned LGTM. Treat these as historical results when
later changes alter the call chain or task ownership.

## Evidence and verification limits

Independent design review required two rounds and resolved three concrete gaps:
pending-probe timeout ordering, unbounded SDK body reads/shutdown, and native
transport diagnostic leakage. The isolated transport spike compiled the native
channel/interceptor seam and exercised HTTP, HTTPS, custom CA, mTLS, hostname
rejection, environment auth suppression, stalled-body cancellation, current-thread
SDK workers and cancellation of a retirement waiter. That spike is design evidence,
not a substitute for tests of the repository implementation.

Repository verification included nine actual transport tests, the final isolated
dynamic run, focused unit/schema tests, repository checks and independent review
rounds on 2026-09-12. Treat these as
historical evidence when re-evaluating a later change. Resource/output, shared readiness and
Controller handler tests exercise schema/redaction/dependencies; the actual
production transport target is `edgion-gateway/tests/otlp_transport.rs`, and the
isolated Controller/Gateway/Collector runner proves live output and hot updates.
Do not interpret compile success or a TCP health gauge as delivery or teardown proof.

## Re-evaluation triggers

Revisit when changing SDK/tonic versions or feature unification, batching limits,
exporter shutdown, logging filters, resolver behavior, Secret/TLS policy, probe
deadlines, source dispatch ordering, or the number/ownership of log consumers.
The product still excludes OTLP/HTTP, tracing/metric export and durable replay.

Round 1 lifecycle review found startup-error drain and rapid remove/recreate
retirement gaps. An armed startup guard now covers all errors after watches start.
The persistent runner and per-key deleting state prevent overlapping generations
until cleanup joins, then apply only the latest deferred desired state. Retiring
counts remain accurate while process shutdown owns the cleanup JoinSet.
Fresh regression evidence is recorded in the task implementation report.

LinkSys auth.secretRef declares all four operator fields (name, namespace, group,
kind) in the structural CRD subtree so Kubernetes cannot prune unsupported group
or kind before typed validation. The focused CRD guard compares this subtree to
SecretObjectReference. Handler apply/delete clear both Secret and cross-namespace
reference managers; valid references to missing or denied Secrets still rebuild
their current wakeup edges. Malformed OTLP identities create no lookup edges.

Actual trace capture must use the production constructor (System DNS). The
integration-only StandardDns injection has Hickory DEBUG events containing queried
hostnames; those are not covered by the OTLP transport-target filter. This was
observed during the 2026-09-12 closure test and remains a separate custom-DNS
observability review item. Exposing operator-selected OTLP DNS requires revisiting
that boundary before claiming the same diagnostic contract.
