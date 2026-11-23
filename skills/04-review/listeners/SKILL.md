---
name: review-listeners
description: Review knowledge for listeners, grouped by decision type.
---

# listeners review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [Process-wide HTTP request concurrency](intentional-design/http-request-concurrency.md)
- [Explicit Linux host tuning baseline](intentional-design/linux-host-tuning-baseline.md)
- [Gateway recreation requires a data-plane restart](intentional-design/gateway-recreation-requires-restart.md)
- [Serial connection admission with optional Redis limits](intentional-design/bounded-serial-connection-admission.md)
- [Graceful upgrade descriptor error boundary](intentional-design/graceful-upgrade-descriptor-error-boundary.md)
- [UDP session table does not defend against spoofed sources](intentional-design/udp-session-table-spoofed-source-limit.md)

## Compatibility constraints

- [Startup-only listener binding, Pingora constraint, and accepted TCP/UDP scope](compatibility-constraints/listener-port-set-frozen-at-startup.md)

## Regression guards

- [Downstream connection admission and exact live counts](regression-guards/downstream-connection-admission.md)
- [Retained listener managers stop serving removed routes](regression-guards/retained-listener-manager-clearing-wire-coverage.md)
- [Gateway Add final-value publication](regression-guards/gateway-add-final-value-publication.md)

- [EGC runtime snapshot publication and lifecycle capture](regression-guards/egc-runtime-snapshot-publication.md)
- [UDP process shutdown and task completion](regression-guards/udp-process-shutdown.md)
- [UDP session retirement follows the owning generation](regression-guards/udp-session-generation-retirement.md)
- [UDP sessions are retired when their pinned endpoint is removed](regression-guards/udp-endpoint-removal-retirement.md)
- [UDP session creation preserves admission and socket failure causes](regression-guards/udp-session-creation-failure-cause.md)
- [UDP upstream response source must match the pinned backend](regression-guards/udp-upstream-response-source.md)
- [Full set direct store update](regression-guards/full-set-direct-store-update.md)
- [Listener Bind Probe — Symmetric With UDP (HTTP/HTTPS/TCP/TLS)](regression-guards/listener-bind-probe-symmetric.md)
- [L4 timeout snapshot publication and connection pinning](regression-guards/l4-timeout-snapshot-publication.md)
