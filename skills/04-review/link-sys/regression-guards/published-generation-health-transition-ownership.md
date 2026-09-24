---
name: published-generation-health-transition-ownership
description: Use when reviewing LinkSys health updates, flip counters, or persistent-runner publication and retirement.
knowledge_type: regression-guards
finding_id: D0007
---

# Published generation owns LinkSys health transitions

## Conclusion and scope

For Redis, Etcd, Elasticsearch, Kafka, and OTLP, only a level change observed by
the currently published persistent-runner generation may increment
`edgion_link_sys_health_flips_total`. Candidate initialization establishes a
baseline without a flip. Replacement establishes the new candidate's current
level as a baseline, deletion emits no synthetic degradation, duplicate results
emit nothing, and an unpublished or retired generation cannot report a flip.

Webhook and LocalFile keep their separate lifecycle and health ownership; this
guard does not move them into the persistent-runner mechanism.

## Rationale and evidence

The original D0007 gap was that the shared Etcd client logged transitions but
could not call Gateway metrics. Inspection also found that provider-local atomic
flags did not make publication ownership transactional: candidates could report
initial recovery and old tasks could race handover.

`edgion-common::link_sys_core::health::PublishedHealth` now owns the lock-free
level plus a short transition mutex, publication bit, and optional binary-owned
reporter. The common crate invokes only the injected callback and does not depend
on Gateway metrics. Gateway construction attaches a callback to
`record_link_sys_health_flip`; Controller Etcd construction attaches none.

`PersistentRunner` calls `PublishedHealth::transition_publication` around its
current-client ArcSwap for first publication, replacement, immediate
unpublication, and deletion. The old and new transition locks stay held across
the swap, preventing an update from landing between reporter ownership changes.
The new value is activated as a baseline without synthesizing a transition.

## Alternatives and limits

Scrape-time comparison cannot observe multiple transitions between scrapes and
cannot assign events safely across generation replacement. Adding Gateway metric
dependencies to `edgion-common` violates binary-owned observability. Provider-local
retired checks alone leave a check-then-update race, so they remain cancellation
hints rather than the ownership barrier.

The reporter runs inside the short transition critical section. It must remain
non-blocking and must not re-enter the same health cell. Publication for one
runner is serialized; callers must not perform opposing concurrent handovers for
the same cells outside that lifecycle.

## Verification and re-evaluation

The Kafka integration recovery oracle checks the current
`edgion_link_sys_health` gauge for the exact LinkSys key and delivery of an
identified access-log record generated after broker restart. Supply a unique
valid W3C `traceparent` and match its canonical
`trace_id`; request paths may be masked and an inbound
`x-trace-id` alone does not establish the canonical identity. Keep normal log
redaction enabled. Match the metric
name and link label independently of global labels such as `service` and their
order. Missing or failed scrapes are not evidence of either health level.
Transition logs are rate-limited and can omit a real recovery; increasing topic
counts alone can reflect old queued records. Neither is a sufficient recovery
oracle. A recovery timeout fails the suite, rather than emitting a warning-only
PASS. Test consumers have container-side deadlines because terminating the host
`docker exec` client does not terminate the container's consumer.

On 2026-09-21, the final trace-ID-based standalone Kafka run passed all eight
assertions, including broker-down health and broker-up current health plus fresh
record delivery. The preceding path-based attempt passed seven assertions but
failed recovery because normal masking replaced the path; that failure verified the nonzero failure
exit, not a production recovery defect. Bash 3.2 matcher fixtures and independent
Astra review cover global-label ordering, missing/wrong health samples, and
masked-path records with correct versus absent or stale trace IDs.

Deterministic tests cover unpublished baselines, duplicate directions,
replacement and deletion handover, concurrent writers, reporter-free Controller
use, Etcd degradation/recovery/retirement, and a runner-level old-versus-current
generation metric assertion. Re-evaluate if a sixth provider joins the persistent
runner, the runner's current-client publication mechanism changes, or reporter
callbacks become blocking.

## Resolution references

- D0007 — Etcd health flip counter gap, expanded after cross-provider analysis
  to the shared persistent-runner publication boundary.
- `edgion-common/src/link_sys_core/health.rs`
- `edgion-gateway/src/link_sys/runtime/runner.rs::tests::handover_reports_only_the_new_published_generation`
- `edgion-gateway/src/link_sys/runtime/store.rs::published_health_tests`
