# Define GatewayClass Process Configuration Ownership

## Meta

- Status: pending
- Type: architecture
- Priority: P2
- Discovered in: Gateway quality campaign B0011

## Evidence

The cluster-synced `EdgionGatewayConfig.spec.server` exposes process-level
settings including worker threads, work stealing, grace periods, upstream
keepalive pool size, and error logging. The Gateway currently consumes other
runtime fields from this resource, but these process-level settings have no
clear application path despite being documented as GatewayClass-adjustable.

## Why deferred

These settings are established while Pingora constructs the process, before
later configuration-sync updates. Multiple GatewayClass resources can also
specify conflicting values. A local consumer would therefore create ambiguous
precedence, restart, and multi-resource behavior.

## Candidate next step

Inventory every process-level field shared between the local Gateway process
configuration and `EdgionGatewayConfig`, then decide whether each field is
local-only, startup-resolved from one authoritative resource, dynamically
applicable, or should be removed from the cluster schema. Document precedence
and restart semantics before changing consumers or schemas.
