---
name: partial-update-inconsistency-windows
description: Use when reviewing `gateway/runtime/handler.rs::full_set` / `partial_update` "rebuild_gateway_tls_matcher and rebuild_listener_gateway_infos are not committed atomically — a concurrent TLS handshake can observe a cross-store epoch mismatch" findings. The cross-store window is an accepted tradeoff. For the intra-store flat/index pair, see `listener-gateway-info-dual-arcswap-merged-snapshot`.
knowledge_type: intentional-design
---

# runtime F-005 — cross-store rebuild ordering in `partial_update` — DESIGN-TRADEOFF

**Status:** ⚠️ accepted tradeoff · 2026-05-28
**Severity:** Low (BadLogic / Concurrency — data-plane hot path observes transient cross-epoch state)
**Scope:** Window 1 (inter-store) only. Window 2 (intra-`ListenerGatewayInfoStore` flat/index) is a separate finding closed by [[listener-gateway-info-dual-arcswap-merged-snapshot]].

## Conclusion

`GatewayHandler::full_set` and `GatewayHandler::partial_update`
(`edgion-gateway/src/runtime/handler.rs`) rebuild two independent ArcSwap-backed
globals **sequentially**, not atomically:

1. `rebuild_gateway_tls_matcher(&gateways)` — single `ArcSwapOption::store` on
   `runtime::matching::tls::GATEWAY_TLS_MATCHER`.
2. `rebuild_listener_gateway_infos(&gateways)` — single `ArcSwap::store` on
   `runtime::store::listener_gateway_info::ListenerGatewayInfoStore` (atomic
   internally — see F013).

Between the two `store()` calls there is a microsecond-scale window in which
a concurrent TLS handshake or request observes the new TLS-matcher epoch
paired with the previous listener-info epoch (or, on the very next request,
the reverse). This stays as-is.

Fix suggestions of the form "wrap `GatewayTlsMatcher`'s `TlsMatcherData` and
`ListenerGatewayInfoStore`'s `Snapshot` under a single
`ArcSwap<RuntimeSnapshot>` so the cross-store swap is one atomic publish"
are **not accepted** under the current call-rate and blast-radius profile.

The `handler.rs` `partial_update` block carries an inline comment pointing to
this entry. `full_set` (`handler.rs:71-77`) has the same rebuild ordering and
the same rationale applies; only `partial_update` carries the comment to
avoid duplication.

## Core Rationale

**1. The window is microseconds long on a control-plane cold path.**

`partial_update` and `full_set` are driven by the controller→gateway
config-sync (`conf_sync` client), debounced at `BATCH_DEBOUNCE = 100 ms` (see
[[partial-update-under-lock-intentional]]). The two
`rebuild_*` calls run back-to-back on a single Tokio worker; each rebuild
builds its maps locally and ends in a single `ArcSwap::store`. The window
between the two `store()` calls is bounded by HashMap iteration + a HashMap
clone — typically microseconds, never blocking, no I/O, no lock acquisition.

**2. The race self-heals on the next request.**

A connection that observes the cross-epoch pair sees either:

- new TLS table + old listener-info: SNI handshake matches a Gateway whose
  routes are still indexed at the old epoch, so route lookup either misses
  (connection rejected) or matches the prior listener configuration. The next
  request after both swaps complete sees the consistent new epoch.
- old TLS table + new listener-info (next request after a handshake done at
  the old TLS epoch): SNI was already matched at the old cert; the route
  lookup at the new listener-info epoch may route differently than the cert
  selected. Result is at worst one transient routing miss.

No persistent state corruption, no leaked resource, no leadership invariant
broken. The `GatewayInfoIndex` and `GatewayTlsMatcher` rebuilt at the next
epoch fully supersede the previous ones; subsequent traffic is consistent.

**3. Merging both stores under one `ArcSwap` is disproportionate.**

A "real" fix would combine `GatewayTlsMatcher::TlsMatcherData` and
`ListenerGatewayInfoStore::Snapshot` into a single
`ArcSwap<RuntimeSnapshot>`. Costs surveyed:

- `match_gateway_tls` is on the TLS handshake hot path
  (`runtime/server/tls_pingora.rs`), called once per inbound connection.
  Routing through a combined snapshot adds one indirection per handshake.
- `get_index` is on the request hot path
  (`gateway/routes/http/proxy_http/pg_request_filter.rs:41`,
  `gateway/routes/tls/passthrough_proxy.rs:376`, plus the per-listener
  `routes_mgr` build paths), called once per request.
- Both globals are `LazyLock<...>` with separate owners. Merging them forces
  a layering change across `runtime/matching/mod.rs`,
  `runtime/store/mod.rs`, and every `routes_mgr` that reads either side.
  `LazyLock` ordering bugs are a recurring pitfall in this codebase (see
  [[oncelock-debug-assert-pitfall]]).
- The two subsystems (matching vs store) are independently re-exported via
  `runtime/mod.rs::pub use`; merging their backing storage breaks that
  separation.

The benefit — closing a microsecond window on a debounced control-plane path
whose worst observable consequence is one transient routing miss — does not
justify the layering disruption or the per-handshake indirection.

**4. The historical "three stores" framing is stale.**

Earlier review rounds named `GatewayConfigStore` as a third interacting
store. F003 deleted that store entirely (the `runtime-F003` finding is
recorded under `skills/04-review/concurrency/`). Only two ArcSwap-backed globals remain on this path,
and only the two rebuilds discussed above can race.

## Criteria that would justify revisiting

- Production trace showing the window producing observable misroutes at a
  frequency that affects SLA (controller pushing Gateway updates faster than
  the per-connection latency budget allows for self-heal).
- A planned redesign that already merges the two stores for an unrelated
  reason (e.g. a unified `RuntimeSnapshot` introduced for incremental
  rebuild or for `EdgionGatewayConfig` v1alpha1 hot-reload), at which point
  Window 1 closes "for free".

## Re-report rejection template

> The cross-store rebuild ordering in `GatewayHandler::full_set` /
> `partial_update` (`rebuild_gateway_tls_matcher` followed by
> `rebuild_listener_gateway_infos`) is an accepted tradeoff — see
> `skills/04-review/concurrency/intentional-design/partial-update-inconsistency-windows.md`.
> The intra-`ListenerGatewayInfoStore` window between `flat_data` and
> `index_data` no longer exists; the store now publishes a single
> `ArcSwap<ListenerGatewayInfoSnapshot>` — see
> `skills/04-review/concurrency/regression-guards/listener-gateway-info-dual-arcswap-merged-snapshot.md`.
> Merging the two stores under one `ArcSwap` is only on the table when a
> real misroute is traced in production or a planned snapshot redesign
> already unifies them.

## Related entries

- [[listener-gateway-info-dual-arcswap-merged-snapshot]] —
  canonical entry for the intra-store flat/index atomic snapshot.
- [[full-set-under-write-lock-intentional]] — sibling
  tradeoff on the conf-sync write-lock side.
- [[partial-update-under-lock-intentional]] — same
  `partial_update` call chain, complementary rationale.
