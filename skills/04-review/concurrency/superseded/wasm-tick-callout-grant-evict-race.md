---
name: wasm-tick-callout-grant-evict-race
description: Historical accepted-risk analysis for the Wasm tick-callout grant eviction race; inactive because a later interleaving disproved its fail-safe-only premise.
status: superseded
finding_id: wasm-tick-callout-DR3
closed: 2026-07-12
knowledge_type: superseded
---

# Former Wasm tick-callout grant eviction accepted risk — SUPERSEDED

## Inactive conclusion

The 2026-07-12 decision accepted a delayed-eviction race on the premise that it
could only disable tick callouts. That premise is false, so this page is not an
active acceptance and must not suppress new findings.

Inside `WasmVmManager::unregister_plugin`, delayed eviction can remove a zero
`refs[key]`, race with a new registration that adds allowlist A to the old
`TickCalloutState`, and then unlink that state. The re-registered plugin remains
live but its grant is absent from the new state. If another live plugin sharing
the runtime key later registers a different allowlist B, `gate()` sees only B
and returns `Enabled` instead of the required A/B `Conflict`. The confirmed
consequence is a cross-plugin allowlist consistency failure, not merely silent
disablement.

## Successor work

The active defect and required atomic lifecycle redesign are tracked by
`tasks/todo/atomic-wasm-plugin-lifecycle-state.md` under stable ID
`wasm-tick-callout-grant-eviction-race`. The task preserves the useful warning
from the former analysis: simply moving `tick_callouts.remove` before the
refcount check is also unsafe. Registration, zero-reference validation, and
grant-state removal need one per-key atomic ownership boundary.

## Residual valid constraints

- Request-phase callouts are unaffected because they use the plugin instance's
  own allowlist.
- Different non-empty allowlists on live plugins sharing one runtime key must
  keep tick callouts disabled; identical allowlists merge with the minimum
  timeout.
- The current evidence does not establish access to an origin that no live
  plugin authorized. It establishes that the documented conflict guard can be
  bypassed.

## Historical reference

- `wasm-tick-callout-DR3` was introduced/reviewed 2026-07-12 and accepted
  under the now-disproved fail-safe-only assumption.
