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
