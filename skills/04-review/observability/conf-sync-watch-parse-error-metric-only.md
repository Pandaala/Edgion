---
name: conf-sync-watch-parse-error-metric-only
description: Decision record for conf-sync F-013 — added per-kind parse-error metric on the gateway watch loop, declined the "N consecutive failures trigger relist" half.
---

# conf-sync F-013 — watch-event parse error: metric added, relist declined

## Conclusion

`ClientCache::start_watch`'s two `serde_json` parse-failure paths
(`from_str(&watch_response.data)` for the events array, and
`from_value::<T>(event_data)` for each per-event payload) in
`edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs` now call
`record_config_event_parse_error(T::kind_name())` before the existing
`tracing::error!` + `continue` / fall-through. The control flow is otherwise
unchanged — there is **no** consecutive-failure counter, **no**
`break 'watch_block` escalation, **no** forced relist.

The metric `edgion_config_event_parse_errors_total{kind=...}` registered in
`observe/metrics/registry.rs` is a labeled free function counter on the cold
path; the `kind` label is sourced from `T::kind_name() -> &'static str` which
is bounded by the closed set of 17 ClientCache<T> instantiations in
`conf_sync/conf_client/config_client.rs` (GatewayClass, Gateway,
EdgionGatewayConfig, HTTPRoute, GRPCRoute, TCPRoute, UDPRoute, TLSRoute,
LinkSys, Service, EndpointSlice, EdgionTls, EdgionPlugins,
EdgionStreamPlugins, BackendTLSPolicy, PluginMetaData, EdgionAcme), so
worst-case cardinality is 17 series.

## Core rationale (why metric only, not metric+relist)

The original F-013 finding proposed *both* a per-kind parse-error counter
*and* a consecutive-failure threshold that breaks out to trigger an outer-loop
relist (the Kubernetes informers "decode-error backoff" pattern). The relist
half was declined because it does not self-heal in the dominant real-world
trigger and only adds noise:

1. **Schema drift (the most common trigger — partial controller upgrade
   changes a resource wire format).** A relist routes through `list_and_reset`,
   which calls the *same* `serde_json::from_str::<Vec<T>>(&list_data.data)`
   decoder; it will fail with `tonic::Status::internal(...)`. The cache stays
   frozen at last-good (list-failure path doesn't reset the cache); the outer
   loop walks the 2s → 32s exponential backoff and keeps trying. End state:
   cache still frozen, plus list-loop log spam. **No self-healing.** The
   newly-added metric already gives operators the alertable signal they
   actually need.
2. **Controller serialization bug producing malformed JSON.** Same as case 1.
3. **One-off frame-level corruption (single event payload bad, rest fine).**
   This is the only case where a relist would actually recover the dropped
   event. But gRPC has already validated the frame at this point — a
   well-formed frame carrying malformed JSON requires an application-layer
   bug; in practice this is vanishingly rare. Accepting that one event for one
   key may remain stale until the next valid update (or operator-triggered
   restart) is a deliberate tradeoff for not paying scenario-1/2 noise costs.

The metric alone resolves the original "no alertable metric, only log spam
that may be rate-limited or aggregated away" defect that F-013 actually called
out in its Impact section. Adding relist on top would not address the same
gap any better.

## Fix suggestions not accepted

- "Add `MAX_CONSECUTIVE_PARSE_ERRORS = N` counter inside `'watch_block` and
  `break 'watch_block` on threshold." — Declined per the rationale above:
  doesn't recover schema drift, adds list-backoff noise.
- "Just bump the log level to `warn!` so it surfaces in operator dashboards."
  — Insufficient. Logs are subject to per-process rate limiting and downstream
  aggregation; the whole point of the F-013 finding is that log-only signals
  are unreliable. A counter is the right channel.
- "Add `kind` as a free-form label populated from a runtime string." — The
  `&'static str` signature on `T::kind_name()` is load-bearing for cardinality
  bounds. Re-reports that propose loosening this signature should be rejected.
- "Use `tracing::error_span!` plus a span-level metric counter." — Doesn't
  apply here (no span context across watch iterations) and would not move the
  cardinality discussion.

## Re-evaluation triggers

- A production incident where a single-event corruption (scenario 3) is the
  confirmed cause of a stale cache, AND restart-only recovery proves
  unacceptable for SLA reasons. Then revisit adding the consecutive-counter
  relist escalation; pair it with a `record_config_event_parse_relist_total`
  counter so the relist storm itself is alertable.
- If a future controller change makes `from_version`-based watch jump-resume
  safe across decoder errors (e.g., per-event `sync_version` allows the
  gateway to relist with a precise floor instead of full reset). Then the
  cost-benefit of escalating after N failures shifts.

## Reference cases

- Closed finding: `tasks/conf/conf-sync-F013-watch-event-parse-error-drops-event-silently.md` (closed 2026-05-28).
- Related pattern: existing transport-level escalation at
  `event_dispatch.rs` watch loop `Err(e)` branch — that path already breaks to
  relist because the *stream* is dead; the parse-error paths do **not** mirror
  it because the stream is healthy and a relist would just route the same
  bad-decoder failure into the list path.
