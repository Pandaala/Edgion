# D0012 — Audit Security-Protect EGC Hot Reload

- Status: ready-for-separate-task
- Canonical task: `tasks/todo/audit-security-protect-egc-hot-reload.md`
- Discovered in: B0066

## Evidence

Several `securityProtect` consumers still read a listener-startup EGC clone, including forwarded-header limits, SNI checks, loopback-upstream policy, TLS proxy logging, and fallback SNI handling. These settings span request, connection, and handshake lifecycles and cannot be safely folded into the request-scoped duplicate-Host correction.

## Boundary

Inventory every consumer and assign the correct immutable generation boundary before changing behavior. Preserve safe defaults, listener/connection/request ownership, binary-owned logging, redaction, and hot-path constraints.
