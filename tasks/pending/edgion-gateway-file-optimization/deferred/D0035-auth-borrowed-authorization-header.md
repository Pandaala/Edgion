# D0035 — Add Borrowed Authorization Header Access

## Meta
- Status: needs-discussion
- Severity: medium
- Discovered in: B0067, BasicAuth audit
- Category: security-performance
- Canonical task: [`tasks/todo/add-borrowed-authorization-header-access.md`](../../../todo/add-borrowed-authorization-header-access.md)

## Evidence
`PluginSession::header_value` constructs an owned String, so affected Authorization-reading auth consumers first copy the reversible Authorization value before parsing. B0067 shortens that String's lifetime after it already exists, but cannot remove the initial allocation or duplicated sensitive bytes without changing the shared session boundary.

## Why deferred
The correction spans the PluginSession trait, Pingora adapter, mocks, and affected Authorization-reading auth consumers. A BasicAuth-only bypass would split the canonical session API and leave sibling authentication plugins unchanged.

## Integration-test impact
None required. Borrowing and allocation ownership are unit/compile-time invariants; existing authentication integration behavior should remain unchanged and frozen.
