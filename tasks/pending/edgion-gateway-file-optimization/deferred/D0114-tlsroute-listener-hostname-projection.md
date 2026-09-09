# D0114 — TLSRoute Listener Hostname Projection

- Status: pending
- Severity: high
- Discovered in: B0204 TLS listener-table analysis
- Canonical task: [Scope TLSRoute effective hostnames to attachments](../../../todo/scope-tlsroute-effective-hostnames-to-attachments.md)

Controller projection merges effective hostnames across listeners into one route
array, which every listener table consumes. With another Gateway accepting the
same SNI on a shared physical listener, a route can match an SNI outside its own
attachment. The same lost association narrows catch-all attachments incorrectly.
Fix requires coordinated Controller/schema/Gateway design; local table storage
optimization may continue while preserving existing semantics.
