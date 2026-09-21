---
name: wasm-tick-callout-grant-evict-race
description: Use when reviewing the Wasm tick-callout grant registry (TickCalloutState / CacheInner.tick_callouts); covers the accepted evict-vs-register race that can drop a fresh grant.
status: accepted-tradeoff
finding_id: wasm-tick-callout-DR3
closed: 2026-07-12
knowledge_type: intentional-design
---

# Wasm tick-callout grant registry: evict/register race can drop a fresh grant — ACCEPTED-RISK

## Conclusion

Inside the delayed-eviction closure (`vm_manager.rs::unregister_plugin`), between
`refs.remove_if(count == 0)` succeeding and `tick_callouts.remove(&key)`, a concurrent
`register_plugin` can `add` its `CalloutGrant` into the old `TickCalloutState` Arc that is
about to be unlinked from the map. The re-registered plugin's later load then creates a
fresh (empty) state via `tick_callout_state()`, so its grant is lost and **tick-phase
callouts stay silently disabled for that plugin instance's lifetime** (request-phase
callouts are unaffected — they arm from the plugin's own allowlist).

Fix suggestions of the form "remove the tick_callouts record before refs.remove_if" or
"take a cross-map lock around eviction" are **not accepted**.

## Core Rationale

- **Fail-safe direction only.** The race can only ever *disable* tick callouts, never arm
  an allowlist nobody granted. The security invariant (no cross-plugin allowlist leakage,
  no union) holds under every interleaving.
- **The window is a few instructions wide** and requires a plugin to re-register the same
  cache key at the exact moment the grace-period eviction fires for it — reload thrash
  already cancels most evictions earlier via the `remove_if(count == 0)` re-check.
- **Reordering makes it worse, not better**: removing `tick_callouts` *before*
  `refs.remove_if` would wipe a *live* plugin's grant whenever the re-check keeps the
  entry (refs > 0), turning a narrow silent-disable race into a common one.
- A cross-map mutex spanning refs/modules/tick_callouts would serialize plugin
  build/drop against the eviction task for a race whose worst case is a degraded
  optional feature, and would be the only cross-shard lock in the manager.
- The failure is observable: the affected module keeps reporting dispatch failures to the
  guest, and the guest-visible error plus absent `edgion_wasm_callout_total{result="ok"}`
  for a module that should be polling makes it diagnosable; a plugin reload heals it.

## Re-evaluation Triggers

- The grant registry gains security-relevant state beyond the allowlist/timeout (e.g.
  per-plugin credentials), where "silently disabled" stops being the worst case.
- Field reports of tick callouts staying disabled after reload storms (the race being hit
  in practice) — then prefer a re-validate-on-load step (plugin re-adds its grant when its
  `get_or_load` completes) over lock reordering.

## Reference

- Introduced/reviewed 2026-07-12 (tick-internal-callout task, opus deep review finding #3,
  judged PLAUSIBLE/LOW). Related confirmed fixes from the same review landed immediately:
  the misleading conflict WARN in `TickCalloutState::add` (compare distinct non-empty
  allowlists, not grant entries) and the `tick_callouts` record leak for never-loaded
  modules (cleanup no longer gated on a `modules` entry existing).
