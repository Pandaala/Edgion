---
name: listener-specificity-duplicated
description: Use when reviewing runtime/store findings of the form "`GatewayListeners::has_more_specific` (listener_gateway_info.rs) and `GatewayListenerConfig::has_more_specific_listener` (config.rs) duplicate the three-tier listener-specificity algorithm; extract a shared free function" — declined in favour of removing the orphaned copy via runtime-F003.
knowledge_type: superseded
---

# Listener Specificity Algorithm Duplicated Across Two Stores — DESIGN-TRADEOFF / SUBSUMED-BY-COMPANION

**Audit date**: 2026-05-28
**Linked issue**: runtime F-002 (Low / Redundancy) — closed as ⚠️ accepted tradeoff, no code change.

> Inactive: runtime-F003 removed the orphaned `GatewayConfigStore` and its duplicate
> specificity method. See [the completed removal](../regression-guards/gateway-config-store-orphaned.md).
> The analysis below is historical and cannot justify retaining a new duplicate.

## Conclusion

The three-tier listener-specificity check (exact > wildcard > catch-all) is implemented twice:

1. `GatewayListeners::has_more_specific` (`edgion-gateway/src/runtime/store/listener_gateway_info.rs:102-132`) — port-naive; lives inside the per-port `GatewayInfoIndex` slice and is the only copy consulted by the data plane (`runtime/matching/route.rs:124,147`, `routes/http/proxy_http/pg_request_filter.rs:72`).
2. `GatewayListenerConfig::has_more_specific_listener` (`runtime/store/config.rs:138-185`, file deleted in runtime-F003) — was port-aware (added an `if config.port != current_port { continue; }` guard to compensate for the all-ports-in-one-map layout). A codebase-wide grep across `src/` returned **zero** production callers; only the sibling `tests/config_test.rs` exercised it.

The two bodies share `hostname_matches_listener`, the `!h.starts_with("*.")` early-return, and the `other_host.len() > current_host.len()` wildcard comparison; the port filter is structural compensation for a different container shape, not algorithmic divergence. The drift risk is real but bounded: any change to the live algorithm in `listener_gateway_info.rs` could silently leave `config.rs` behind, but the resulting divergence would only mis-grade the dead test-only path — production behaviour cannot regress because production never calls the `config.rs` copy.

Fix suggestions of the form "extract `is_more_specific_listener(other_host: &str, current_hostname: Option<&str>) -> bool` into the `matching` module and call it from both wrappers" are **not accepted** as a standalone change. The whole `GatewayConfigStore` (660 LOC) is already tracked for removal under runtime-F003 (`gateway-config-store-orphaned.md`); deleting the orphaned copy as part of that work eliminates the duplication at no cost. Adding an indirection layer to a function the project is about to delete is pure churn.

## Core Rationale

**1. The companion finding deletes the duplicate at zero cost**

Runtime-F003 records that `GatewayConfigStore` has five orphaned query methods (`get_listener_config`, `has_listener`, `has_host`, `has_more_specific_listener`, `load_gateways`) — none called from `src/` outside the file itself — plus a `gateway_count` admin-metrics surface that can trivially be re-rooted on `ListenerGatewayInfoStore`. F002 explicitly notes in its own Fix section: "If `GatewayListenerConfig::has_more_specific_listener` is removed as part of F-003 (`GatewayConfigStore` elimination), this finding dissolves automatically." Doing the F002 refactor first means the F003 work then deletes the freshly-extracted helper (or has to migrate its single remaining caller back into `listener_gateway_info.rs`). That is negative-value sequencing.

**2. Drift-prevention argument fails because the drifting half is unreachable**

The general project rule in [`style-consolidation-thresholds.md`](../../review-method/intentional-design/style-consolidation-thresholds.md) §(a) requires *real maintenance cost or reading cost evidence* before extracting a helper for "looks duplicated" cases. The standard escape hatch for duplication review — "extraction is for drift prevention" — assumes both copies are reachable. Here the second copy is exercised only by unit tests in the same module. A future drift would surface as a unit-test failure, not as a production regression, which is the same failure mode as deleting both the function and its tests. The drift risk is therefore an artefact of keeping dead tests alive, not a property worth paying refactor cost to prevent.

**3. The port-filter difference is data-layout compensation, not algorithm divergence**

`GatewayListeners` (per `GatewayInfoIndex`) is *itself* a per-port slice — the container guarantees all entries share the current port, so `has_more_specific` does no port check. `GatewayListenerConfig` flattens all ports into one `HashMap`, so `has_more_specific_listener` must filter on port at iteration time. A shared free function that abstracts over both would have to either (a) demand the caller pre-filter the input iterator, which moves the port guard back to each caller (no helper benefit), or (b) accept a port parameter and a "do I need to filter" flag, leaking the per-store data-layout difference into the helper's signature. Neither shape improves on inline duplication while the orphaned store still exists; once F003 deletes the store, the question evaporates.

**4. Signature-bloat-dominates-body antipattern (same shape as plugin-runtime-F003)**

The extracted helper would be ~25 lines (the loop body + the wildcard comparison). The signature carries an `other_host: &str` and a `current_hostname: Option<&str>` and returns `bool` — modest by itself, but the caller-side prose to pre-filter the iterator and convert `&Arc<ListenerConfig>` / `&ListenerEntry` into the helper's input shape adds back what the body removed. See [`add-from-builders-duplication.md`](../../plugin-runtime/intentional-design/add-from-builders-duplication.md) §3 for the canonical declined case.

## Fix Suggestions Not Accepted

- "Extract `is_more_specific_listener(other_host: &str, current_hostname: Option<&str>) -> bool` into the `matching` module and call it from both wrappers" — adds an indirection layer to a function F003 will delete; negative-value sequencing.
- "Make `has_more_specific_listener` delegate to `has_more_specific` after pre-filtering by port" — keeps the dead wrapper alive, requires building a new iterator that the F003 deletion will also remove.
- "Leave both copies and add a `// Keep these in sync` comment on each" — comment-rot bait; the comment becomes stale the moment F003 lands and nobody updates it.
- "Drop the `config.rs` copy and its tests now, independently of F003" — would partially anticipate F003 but leaves the `GatewayConfigStore` write path in `handler.rs` (the actual cost source identified by F003) untouched; better to handle in one coordinated change than to fragment the deletion across two PRs.

## Re-evaluation Triggers

Re-open this decision only if:

- Runtime-F003 is rejected (e.g., a planned feature actually requires `GatewayConfigStore`'s per-listener config lookup). At that point the duplication becomes long-lived and extracting a shared helper is appropriate — the rationale above is contingent on F003 deleting `GatewayConfigStore` outright.
- The `listener_gateway_info.rs` specificity algorithm gains a new tier (e.g., a fourth precedence level) **before** F003 lands. At that point either propagate the change manually to `config.rs` to keep its tests honest, or accept that the tests will start mis-grading and document it.
- A third call site for the same specificity check appears outside both `listener_gateway_info.rs` and `config.rs`. Three copies crosses the threshold in [`size-and-similarity-heuristics.md`](../../review-method/intentional-design/size-and-similarity-heuristics.md) and would justify the helper independent of F003.

## Reference Cases

- Issue file: `tasks/runtime/runtime-F002-has-more-specific-logic-duplicated.md` (closed 2026-05-28, no code change).
- Companion finding (drives the actual deletion): `tasks/runtime/gateway-config-store-orphaned.md`.
- Source: `edgion-gateway/src/runtime/store/listener_gateway_info.rs:102-132`; the second copy formerly lived at `runtime/store/config.rs:138-185` (file deleted in runtime-F003).
- Project-level "looks duplicated needs evidence" rule: [`style-consolidation-thresholds.md`](../../review-method/intentional-design/style-consolidation-thresholds.md).
- Three-precondition checklist for line-similarity refactors: [`size-and-similarity-heuristics.md`](../../review-method/intentional-design/size-and-similarity-heuristics.md).
- Signature-bloat-dominates-body antipattern: [`add-from-builders-duplication.md`](../../plugin-runtime/intentional-design/add-from-builders-duplication.md).
- Cross-route consistency matrix (for context on the live specificity path): [`route-architecture-consistency.md`](../intentional-design/route-architecture-consistency.md).

## Resolution references

- `runtime-F002` — 2026-05-28 (⚠️ accepted tradeoff).
