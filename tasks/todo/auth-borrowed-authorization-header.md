# Add Borrowed Authorization Header Access

## Meta
- Status: pending
- Severity: medium
- Category: security-performance

## Evidence
`PluginSession::header_value` constructs an owned String, so affected Authorization-reading auth consumers first copy the reversible Authorization value before parsing. Existing cleanup shortens that String's lifetime after it is created but cannot remove the initial allocation or duplicated sensitive bytes without changing the shared session boundary.

## Why separate
The correction spans the PluginSession trait, Pingora adapter, mocks, and affected Authorization-reading auth consumers. A BasicAuth-only bypass would split the canonical session API and leave sibling authentication plugins unchanged.

## Integration-test impact
None required. Borrowing and allocation ownership are unit/compile-time invariants; existing authentication integration behavior should remain unchanged and frozen.
