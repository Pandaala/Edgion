---
name: cross-kind-config-not-atomic-extensionref-torn-state
description: Use when reviewing findings that compare Edgion against Envoy/Istio xDS atomic snapshots (SnapshotCache) and claim "cross-kind config updates are not atomic / torn state between Gateway and Route observable per request / HTTPRoute applied before its referenced EdgionPlugins arrives / ExtensionRefFilter resolves a dangling plugin reference during the propagation window / per-kind ArcSwap/RwLock with no shared epoch / 100-200ms debounce widens the window / security plugins bypassed during config apply / need a config_generation/epoch like xDS versioned snapshots". Covers why per-kind independent sync producing a bounded, self-healing, fail-closed window on the HTTPRoute -> EdgionPlugins ExtensionRef path is an accepted tradeoff, not a defect, and why the epoch/generation fix is a large cross-layer feature.
status: accepted-tradeoff
finding_id: resource-02
closed: 2026-06-14
knowledge_type: intentional-design
---

# Cross-kind config updates are not atomic — HTTPRoute -> EdgionPlugins ExtensionRef torn-state window — ACCEPTED-TRADEOFF (bounded, self-healing, fail-closed; matches Envoy/Istio default baseline)

## Conclusion

Edgion syncs each resource kind over an independent gRPC watch stream with its own
`ClientCache<T>`, its own 100ms `BATCH_DEBOUNCE`, its own ConfHandler, and its own
per-kind runtime store (`PluginStore` behind `ArcSwap`, `GlobalHttpRouteManagers`
behind per-listener `ArcSwap`, `GatewayStore` behind `RwLock`). There is no shared
cross-kind transaction or epoch. When one logical config change spans
`HTTPRoute` + the `EdgionPlugins` it references via `ExtensionRef`, the two kinds
land independently, so there is a transient window where the route is live but the
referenced plugin bundle has not yet arrived in `PluginStore`.

This is an **accepted design tradeoff**, not a defect and not a narrow review fix.
The proposed "assign a monotonic `config_generation` shared across kinds in the same
operator transaction + gate `ExtensionRefFilter` resolution on the route's
sync_version reaching that generation + a stale-plugin fallback state machine" is a
large cross-layer feature (Controller generation assignment + Gateway-side
generation gating + a new pending-reference state machine) and belongs in
`tasks/todo/` via brainstorming if ever prioritized.

## Core Rationale

**1. The actual runtime behavior is fail-closed and self-healing, not a security bypass**

The finding frames the window as "some plugins fail open (skip the filter), some
fail closed" and warns of a "brief security-plugin bypass window violating
compliance". That premise does not hold for the ExtensionRef path. When the
referenced `EdgionPlugins` is not yet in the store, `prepare_execution`
(`edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs:131-146`)
pushes `"extension ref not found"` and returns `Err(())`, which every caller maps
to a **transient HTTP 500** (`run_extension` -> `ErrResponse { status: 500 }`;
`run_extension_async` request stage -> 500 + JSON body + `ErrTerminateRequest`;
`run_upstream_response` -> `ErrResponse { status: 500 }`). The whole referenced
bundle is unavailable -> 500; individual plugins inside it are never selectively
skipped. So the window **fails closed** (transient 500), it does not let a request
through with security plugins bypassed.

**2. No route rebuild is needed — resolution is per-request against the live store**

`ExtensionRef` targets are resolved at request time against the **current**
`PluginStore`, not baked into the route table at build time:
`pg_request_filter.rs:186-192` calls
`resolve_route_ext_refs(&plugin_store, unit.plugin_runtime.ext_ref_keys())` on the
live store the first time a request touches the route. The moment the lagging
`EdgionPlugins` Watch event is applied (store swapped via `ArcSwap`), the next
request resolves the reference correctly. There is no poisoned route table and no
operator action required — the window self-heals on plugin arrival.

**3. The window is bounded and not below the Envoy/Istio default baseline**

The exposure is bounded by network jitter + debounce (~100-200ms across two
independent debounce tasks). Envoy's atomic `SnapshotCache` holds within a single
xDS management server controlling delivery order; Edgion's structural equivalents
are control-plane ingest validation (so a *malformed* EdgionPlugins never ships —
see `no-ack-nack-control-plane-validation.md`, sync-01) plus the startup
`wait_for_ready` barrier (so process-start traffic does not flow until every kind
finished its first List — see `cross-kind-dependency-ordering-not-enforced.md`,
sync-02). For runtime incremental updates, Envoy/Istio data planes likewise serve
a transient error when a route references config that has not yet propagated; the
eventual-consistency edge exists there too and is also bounded by propagation
latency. Edgion is at parity with the default baseline here, not behind it.

This finding is the EdgionPlugins-plugin-reference sibling of the cross-kind
cluster: `sync-01` (no ACK/NACK), `sync-02` (no dependency ordering — Service /
EndpointSlice / TLS cert), and the controller-side `controller-04` (no cross-kind
snapshot atomicity). It is distinct from `sync-02`: sync-02 covers the
Service/EndpointSlice and TLS cert ordering windows (transient 503 on zero
backends); resource-02 covers the HTTPRoute -> EdgionPlugins ExtensionRef window
(transient 500 on unresolved plugin reference).

## Fix Suggestions Not Accepted

- "Assign a monotonic `config_generation` (or reuse `sync_version`) shared across
  all kinds in the same operator transaction and gate `ExtensionRefFilter`
  resolution on the plugin store reaching that generation" — a cross-layer epoch
  mechanism (Controller transaction grouping + Gateway-side per-kind generation
  bookkeeping + read-side gating) with medium risk; promote to `tasks/todo/` via
  brainstorming if ever prioritized.
- "Make `ExtensionRefFilter` distinguish 'plugin never existed' from 'plugin not
  yet arrived' and apply a per-request retry or a stale-plugin fallback policy" —
  a new pending-reference state machine plus a fallback-policy surface is wide
  impact; the live-store per-request resolution already self-heals on the next
  request, and a fallback that serves the *previous* plugin set risks running stale
  security policy, which is worse than a bounded transient 500.
- "Treat the torn-state window as a high-severity compliance violation / security
  bypass" — rejected on the facts: the ExtensionRef path fails closed (transient
  500), it does not bypass security plugins (Rationale 1).

## Re-evaluation Triggers

Re-open (as a `tasks/todo/` feature, not a review fix) only if:

- A production incident shows the runtime incremental-update window causing a
  measured, non-transient failure (e.g. routes returning 500 on a missing plugin
  well beyond Watch-propagation latency) that multi-replica + readiness probes
  cannot cover.
- The project adds a cross-kind generation/epoch model for another reason, making
  ExtensionRef generation-gating an incremental rather than greenfield change.
- The conf-sync protocol moves to a single multiplexed stream with server-controlled
  ordering, at which point atomic cross-kind apply becomes cheap to enforce.

## Reference Cases

- `tasks/review-envoy/resource-02-cross-kind-no-atomic-snapshot.md` (closed 2026-06-14, ⚠️ accepted-tradeoff)
- `edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs:131-146` (`prepare_execution` -> `Err(())` -> transient 500 on unresolved reference; fail-closed)
- `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs:186-192` (`resolve_route_ext_refs` against the live `PluginStore` per request — self-heals on plugin arrival, no route rebuild)
- `edgion-gateway/src/conf_sync/cache_client/cache_data.rs:11-17` (`BATCH_DEBOUNCE` 100ms per-kind; independent `CacheData`)
- `edgion-gateway/src/plugins/http/plugin_store.rs:134-135` (`PluginStore` behind `ArcSwap<PluginMap>`)
- `skills/04-review/conf-sync/intentional-design/cross-kind-dependency-ordering-not-enforced.md` (sync-02 — sibling cross-kind ordering tradeoff; Service/EndpointSlice/TLS window)
- `skills/04-review/conf-sync/intentional-design/no-ack-nack-control-plane-validation.md` (sync-01 — sibling Envoy-comparison config-sync tradeoff; control-plane ingest validation)
- `tasks/review-envoy/controller-04-no-cross-kind-snapshot-atomicity.md` (controller-side sibling of this finding)
