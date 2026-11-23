---
name: http-request-concurrency
description: Scope of process-wide HTTP request counting at Gateway admission.
knowledge_type: intentional-design
---

# Process-wide HTTP request concurrency

For arch-02, the project declined a fixed Gateway-wide in-flight HTTP request
limit. The process [connection counter](../../../../edgion-gateway/src/runtime/server/downstream_connections.rs)
holds a slot per socket, and the [H2 stream setting](../../../../edgion-gateway/src/runtime/server/downstream_h2.rs)
limits concurrency on each connection. Operators therefore cannot set a single
request ceiling across connections and listeners.

A shared request budget could bound the number of active exchanges when an
upstream slows down. It would also let long-lived streams on one route exhaust
the slots for short requests on another, even with spare process capacity.
The proposal identified no deployment requiring that behavior or measured
concurrency threshold at which it would protect the Gateway.

Reconsider if an operator needs a shared per-instance request budget or a
representative load test identifies a concurrency threshold that justifies
rejecting requests across routes. [Memory-pressure shedding](../../../../tasks/todo/gateway-overload-manager.md)
remains a separate proposal.
