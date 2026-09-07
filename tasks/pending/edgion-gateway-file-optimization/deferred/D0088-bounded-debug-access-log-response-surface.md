# D0088 — Define a Bounded Debug Access Log Response Surface

## Meta

- Status: needs-discussion
- Severity: medium
- Discovered in: B0125 Gateway API debug access-log analysis
- Category: bounded-observability
- Canonical task: [`tasks/todo/define-bounded-debug-access-log-response-surface.md`](../../../todo/define-bounded-debug-access-log-response-surface.md)

## Evidence

The filter serializes a broad non-terminal `AccessLogEntry` snapshot into one downstream response header without a hard serialized-byte cap. A sufficiently large bounded collection of plugin diagnostics or upstream attempts can still exceed downstream or protocol header limits. Serialization and header-set failures are currently ignored, and truncating JSON would create an invalid payload.

## Why deferred

A safe correction must decide whether the plugin remains generally configurable, becomes explicitly development-only, adopts a compact dedicated schema, or is removed. It must also define protocol-aware limits and deterministic error behavior while reusing D0068's canonical response-header mutation/error policy rather than introducing a competing mechanism.

## Integration-test impact

Focused unit tests should own exact limits, valid JSON, and error decisions first. D0087 owns current wire coverage; the final bounded-surface design should extend standalone protocol coverage if it changes the public response contract, without modifying existing frozen integration files.
