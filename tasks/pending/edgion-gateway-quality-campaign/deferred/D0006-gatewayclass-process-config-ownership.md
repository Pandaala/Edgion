# D0006 — GatewayClass Process Configuration Ownership

## Meta

- Status: ready-for-separate-task
- Severity: medium
- Discovered in: B0011, `edgion-gateway/src/cli/pingora.rs`
- Category: configuration architecture

## Evidence

`EdgionGatewayConfig.spec.server` advertises process-level settings such as
worker threads, work stealing, grace periods, upstream keepalive pool size,
and error logging. Gateway consumption currently covers only request/runtime
settings such as compression and downstream keepalive request limits; the
process-level fields have no clear consumer even though user documentation
describes GatewayClass-level adjustment.

## Why deferred

Pingora process settings are fixed during server construction, while multiple
GatewayClass configurations can arrive later through configuration sync and
may disagree. Correcting the mismatch requires an explicit ownership,
precedence, and restart policy rather than opportunistically wiring fields into
the active process.

## Canonical task

- `tasks/todo/define-gatewayclass-process-config-ownership.md`
