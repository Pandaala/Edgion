# D0007 — Etcd LinkSys Health Flip Counter

## Meta

- Status: ready-for-separate-task
- Severity: low
- Discovered in: B0035, `edgion-gateway/src/link_sys/runtime/store.rs`
- Category: observability architecture

## Evidence

The Gateway now samples the published Etcd client for
`edgion_link_sys_health`, and the shared Etcd client logs health transitions.
Unlike the Gateway-owned Redis, Elasticsearch, Kafka, and OTLP health paths,
Etcd does not emit `edgion_link_sys_health_flips_total`. Multiple Etcd
degradation and recovery transitions between two scrapes therefore remain
invisible to the flip counter.

## Why deferred

The Etcd client is owned by `edgion-common`, which must not depend on Gateway
metrics. Closing the gap requires a binary-owned transition-reporting seam or
Gateway-side transition state with defined generation, initialization, and
duplicate-event semantics. It is not safe to add a metrics dependency to the
shared client or infer transitions opportunistically during scrape sampling.

## Canonical task

- `tasks/todo/add-etcd-linksys-health-flip-reporting.md`
