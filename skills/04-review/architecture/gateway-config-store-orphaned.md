---
name: gateway-config-store-orphaned
description: Use when reviewing runtime/store findings of the form "GatewayConfigStore writes a per-Gateway listener-config snapshot but the data plane reads ListenerGatewayInfoStore" or "has_more_specific is duplicated across runtime stores" — the orphaned store was deleted.
status: fixed
finding_id: runtime-F003
closed: 2026-05-28
---

# GatewayConfigStore Orphaned Secondary Store Removed — FIXED

## Conclusion

`GatewayConfigStore` (formerly at `core/gateway/runtime/store/config.rs`, ~390 LOC, file deleted in this fix) wrote a two-layer `ArcSwap<HashMap<gateway_key, GatewayListenerConfig>>` from `handler.rs::full_set` / `partial_update` on every Gateway create / update / delete (clone-and-`ArcSwap::store`, cost proportional to Gateway count), but its five core query methods — `get_listener_config`, `has_listener`, `has_host`, `has_more_specific_listener`, `load_gateways` — had zero callers anywhere under the `src/` tree outside the store's own `#[cfg(test)]` block. The store was removed.

Re-reports of "GatewayConfigStore exists but data plane reads ListenerGatewayInfoStore" / "has_more_specific is duplicated across runtime stores" are **not accepted**; verify the absence of `runtime/store/config.rs` and the single `has_more_specific` implementation in `listener_gateway_info.rs` before re-reporting.

## Core Rationale

**1. The new index fully superseded the old store**

The data-plane hot path consumes `ListenerGatewayInfoStore::GatewayInfoIndex` exclusively (see `core/gateway/runtime/matching/route.rs`); the doc comment in `core/gateway/runtime/gateway_info.rs` already noted the index was introduced "from GatewayConfigStore to support hot-reload" — i.e. the new store fully superseded the old one but the latter was never removed.

**2. Only live read repointed to the primary store**

The only live read of `GatewayConfigStore` was `gateway_count()` from the admin `/store_stats` endpoint (`core/gateway/api/mod.rs` `store_stats` handler), repointed to a new `GatewayStore::count()` on `core/gateway/runtime/store/gateway.rs` that returns `self.gateways.len()` under the existing `parking_lot::RwLock<GatewayStore>` (no integer-count behaviour change; `GatewayStore` is updated immediately before `GatewayConfigStore` was in the old handler, so the admin count cannot be staler).

**3. Scope of deletion**

Removing the three `config_store.*` write calls drops 4 sites in `handler.rs` (`full_set` once, `partial_update` add/update/remove arms); the module and its sibling `tests/config_test.rs` are deleted; `core/gateway/runtime/store/mod.rs` drops the `config` declaration and its re-exports (`GatewayConfigStore`, `GatewayListenerConfig`, `ListenerConfig`, `get_global_gateway_config_store`, plus the re-export of `GatewayInfo` which is now sourced from `core/gateway/runtime/gateway_info.rs` directly — no external caller used `store::GatewayInfo`); `core/gateway/runtime/mod.rs` drops `get_global_gateway_config_store` from its `pub use`; `core/gateway/runtime/matching/route.rs` rewrites the `use ...store::config::GatewayInfo` import to `use crate::core::gateway::runtime::gateway_info::GatewayInfo`.

**4. Collateral: runtime-F002 dissolves**

`runtime-F002` ("listener specificity algorithm duplicated in two types") dissolves automatically — the `config.rs::has_more_specific_listener` copy that F-002 was tracking is gone, leaving the live `listener_gateway_info.rs::GatewayListeners::has_more_specific` as the single implementation. F-002's CLOSED-FINDINGS row predicted this sequencing ("if the function is removed as part of F-003, this finding dissolves automatically") and is now historically correct as-written.

## Fix Suggestions Not Accepted

- Resurrecting `GatewayConfigStore` for a future Gateway-keyed listener-config snapshot — the right answer would be a fresh purpose-built store, not reviving the orphan.

## Re-evaluation Triggers

Re-open only on:

- A planned feature explicitly needing a Gateway-keyed listener-config snapshot independent of `ListenerGatewayInfoStore` — in which case the right answer is a fresh purpose-built store, not resurrecting the orphan.
- A `/store_stats` consumer depending on a count that is *strictly* a snapshot of the deleted secondary store rather than the primary `GatewayStore` (no such consumer exists today — `cache_residue` validator and `run_conf_sync_*.sh` only assert "count is non-zero / matches expected").

## Reference Cases

- Source: 2026-05-28 gateway runtime store review (runtime-F003).
- Companion finding: `architecture/listener-specificity-duplicated.md` (dissolved by this deletion).
- Live store: `src/core/gateway/runtime/store/listener_gateway_info.rs`; deleted file was `config.rs` in that same `runtime/store/` directory.
