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

In the former `WasmVmManager::unregister_plugin`, delayed eviction could remove a zero
`refs[key]`, race with a new registration that adds allowlist A to the old
`TickCalloutState`, and then unlink that state. The re-registered plugin remains
live but its grant is absent from the new state. If another live plugin sharing
the runtime key later registers a different allowlist B, `gate()` sees only B
and returns `Enabled` instead of the required A/B `Conflict`. The confirmed
consequence is a cross-plugin allowlist consistency failure, not merely silent
disablement.

## Successor ownership decision

The user approved private configured-plugin runtime ownership under stable ID
`wasm-tick-callout-grant-eviction-race`. The successor
[runtime isolation rule](../../wasm/regression-guards/wasm-module-cache-key-runtime-config.md)
replaces global runtime/refcount/grant maps and delayed eviction with common
plugin Arc ownership. Each instance uses its own immutable grant for requests
and ticks. The former cross-plugin conflict policy is removed, not silently
relaxed inside a still-shared runtime. The 2026-09-23 replacement passed 226
selected Gateway Wasm tests, the generic task-scope test, workspace check, Clippy,
and independent read-only review.

The historical warning remains useful for any future shared design: merely
moving `tick_callouts.remove` before the refcount check is unsafe too. A shared
registry would again need a genuinely atomic lifetime boundary.

## Historical evidence limits

- Request-phase callouts were unaffected because they used the plugin instance's
  own allowlist.
- The former sharing contract required different non-empty allowlists to disable
  ticks and identical lists to merge with the minimum timeout. That contract
  explains the defect; it is not the current plugin-local authorization policy.
- The current evidence does not establish access to an origin that no live
  plugin authorized. It establishes that the documented conflict guard can be
  bypassed.

## Historical reference

- `wasm-tick-callout-DR3` was introduced/reviewed 2026-07-12 and accepted
  under the now-disproved fail-safe-only assumption.
