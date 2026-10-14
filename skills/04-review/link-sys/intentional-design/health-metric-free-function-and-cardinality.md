---
name: health-metric-free-function-and-cardinality
description: Use when reviewing LinkSys health-metric findings; covers why `edgion_link_sys_health` uses a free-function `gauge!`/`counter!` (not a OnceLock struct handle) and why the per-`link` label is cardinality-safe.
status: accepted-tradeoff
finding_id: link-sys-kafka-health-02
closed: 2026-06-16
knowledge_type: intentional-design
---

# LinkSys health metric: free-function emit + per-link label — NOT-AN-ISSUE

## Conclusion

`record_link_sys_health(name, healthy)` (gauge `edgion_link_sys_health`) and
`record_link_sys_health_flip(name, to_healthy)` (counter `edgion_link_sys_health_flips_total`) in
`edgion-gateway/src/observe/metrics/registry.rs` use free-function `gauge!`/`counter!`
with a per-provider `link` label (and a fixed `direction` label on the counter).
The gauge is sampled from the published runtime client immediately before each
`/metrics` render; probe and operation tasks update their own health flags but
do not write the shared gauge. This prevents a retired generation's late result
from leaving the replacement's gauge at the old value (ISSUE-006).
The counter is emitted through a Gateway-owned reporter attached to the shared
`PublishedHealth` cell. Its publication bit is transferred by the persistent
runner together with the current-client swap, so only a real transition of the
published generation is counted.
The scrape covers all five persistent-runner providers: Redis, Etcd,
Elasticsearch, Kafka, and OTLP. Webhook publication uses the unified runtime table;
its managed outbound client owns health state. Webhook is not a
persistent-runner gauge source.
Suggestions to register them as `OnceLock` handles on `GatewayMetrics`, or to drop the `link` label
over cardinality fears, are **not accepted**.

## Core Rationale

**1. Free-function `gauge!`/`counter!` fits the bounded metrics paths.**

`record_cb_ejection`/`record_cb_recovery` and
`record_link_sys_es_bulk_channel_full` also use the free-function form. Where a
provider implements LinkSys flip counters, they fire on state changes; the gauge
is refreshed once per published provider per scrape. Neither is per-request, so
handle lookup is outside the data-plane hot path. The first scrape registers a
published provider's series. Adding `GatewayMetrics` struct fields would not
improve request-path cost.

The `/metrics` handler calls `record_published_link_sys_health` before rendering.
That function reads each provider runner's `current` handle, as data-plane
callers do, and samples its `healthy()` value for Redis, Etcd, Elasticsearch,
Kafka, and OTLP. An unpublished candidate and a retired old client cannot
directly write the gauge or the flip counter. A scrape racing a handle swap may sample either
published generation; the next scrape samples the then current handle. Health
flip reporters remain binary-owned, and transition logs stay with each
provider's health path; scrape-time sampling does not synthesize transitions.
The five persistent-runner providers share the same publication-aware state.
Candidate initialization establishes a baseline without a counter event;
replacement disables the old reporter and activates the new baseline around the
ArcSwap mutation; deletion disables reporting before unpublication; duplicate
observations and later retired-generation updates are ignored by the counter.

**2. The `link` label requires a validated provider identity.**

One series per configured LinkSys provider — the same bounded shape as the existing `service`
(circuit breaker / LB degrade) and `kind` (config parse) labels in this file. `direction` is a fixed
2-value set (`recovered`/`degraded`). OTLP process references are user supplied, so provider count alone was not a
sufficient safety argument. Validate namespace/name syntax and lengths before
constructing senders or health identities. The former inline OTLP endpoint label
violated this precondition (D0023); dedicated LinkSys ownership removes that path.
The series domain is configured, validated provider keys, not arbitrary URIs.

**3. Stale series on provider deletion is a known, accepted behavior.**

A deleted provider can leave `edgion_link_sys_health{link="..."}` at its last value
until configured idle eviction removes the series. With idle eviction disabled,
it can remain for the process lifetime. The shared production recorder supports
idle eviction for all metric kinds; the earlier blanket non-expiration claim was
incorrect. This acceptance is specific to LinkSys; it does not justify stale
current-state metrics in other mechanisms. The series count is bounded by
providers ever seen.

Redis, Etcd, Elasticsearch, and Kafka leave the last sampled value on deletion.
Before ISSUE-006, OTLP instead explicitly set its gauge to `0` when its runner
was deleted. Removing that direct write makes a deleted healthy OTLP client
retain its last `1` until eviction or process exit. This is an observable change
of the scrape-time ownership fix, not historical OTLP behavior. A newly
published client with the same key replaces the sampled value on its next scrape.
Etcd follows that same scrape/stale-series boundary: deletion stops future
sampling but does not actively clear the existing series.

The former LB gauge was incorrectly listed here as a cold-path precedent: its
writer ran during request selection. It has been replaced by a cached event
Counter; see [Service degrade observability](../../lb/regression-guards/degrade-selection-counter-ownership.md).

## Fix Suggestions Not Accepted

- "Register these as `OnceLock<Gauge>`/`OnceLock<Counter>` on `GatewayMetrics`" — no
  data-plane hot-path benefit for a scrape-time gauge and transition counter.
- "Drop the `link` label to avoid cardinality blowup" — cardinality is bounded by provider count;
  dropping it makes per-provider health undiagnosable.
- "Expire the stale series on provider deletion" — out of scope for the
  scrape-time ownership correction; idle eviction remains the shared recorder's
  mechanism for removing abandoned per-key series.

## Re-evaluation Triggers

Re-open only if:

- A future caller emits one of these metrics on a per-request (hot) path — then reassess handle
  registration and label cost.
- The metric exporter gains a direct observable-gauge API or published-client
  sampling becomes too expensive at the configured provider cap — revisit the
  scrape-time update mechanism while preserving published-generation ownership.
- A label is added whose value is operator/user-controlled or otherwise unbounded.

## Reference Cases

- 2026-06-16 Kafka health-recovery fix (spec `docs/superpowers/specs/2026-06-15-kafka-health-recovery-design.md`).
- Precedent: `edgion-gateway/src/observe/metrics/registry.rs` (`record_cb_ejection`, `record_cb_recovery`, `record_link_sys_es_bulk_channel_full`).
- `ISSUE-006` — corrected late retired-generation gauge writes by sampling the
  published client at scrape time; see `runtime/store.rs::record_published_link_sys_health`.
- `D0007` — added binary-owned Etcd flip reporting and generalized exact
  published-generation transition ownership across all five persistent providers.
- `runtime/store.rs::published_health_tests::published_etcd_client_is_sampled_without_network_init`
  — verifies Etcd's published generation is sampled without connecting.
- [`Published generation owns LinkSys health transitions`](../regression-guards/published-generation-health-transition-ownership.md)
  — lifecycle and exact-counting regression boundary.
- [Webhook publication stays outside the generic lifecycle](../regression-guards/webhook-publication-outside-generic-lifecycle.md)
  — separate Webhook manager ownership.

## Classified dynamic lifetime

The free functions use Common `dynamic_gauge!` / `dynamic_counter!` interfaces.
The `link` identity remains dynamic and counts toward lifetime admission; provider
names are not a closed fixed domain. Scrape sampling publishes active runtime
health before canonical rendering. Removed providers have no sample source, and
idle export epochs expire without refunding admission. Keep published-generation
flip ownership unchanged. Never retain a raw exporter handle across dynamic idle
expiration.
