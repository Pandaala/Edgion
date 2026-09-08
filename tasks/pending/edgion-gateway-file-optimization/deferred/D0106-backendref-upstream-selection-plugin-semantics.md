# D0106 — BackendRef Upstream-selection Plugin Semantics

- Status: pending
- Severity: high (configuration/runtime consistency)
- Discovered in: B0188 HTTP upstream-peer analysis
- Canonical task: [Define backendRef upstream-selection plugin semantics](../../../todo/define-backendref-upstream-selection-plugin-semantics.md)

BackendRef ExtensionRefs can contain request plugins that publish upstream jumps,
but they execute after the current sole jump-dispatch point. The jump is not
selected consistently while residual state can still affect later URI or timeout
processing. Resolution requires one explicit product contract: reject such
plugins at backendRef scope, or define safe late selection, recursion, response
plugin ownership, access-log identity, and retry semantics.
