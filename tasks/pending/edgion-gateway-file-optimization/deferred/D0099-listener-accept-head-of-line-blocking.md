# D0099 — Listener Accept Head-of-line Blocking

- Status: pending
- Severity: high (availability and accept throughput)
- Discovered in: B0147 connection-filter bridge analysis
- Canonical task: [Avoid listener accept head-of-line blocking](../../../todo/avoid-listener-accept-head-of-line-blocking.md)

Pingora's default single listener task per file descriptor awaits Edgion's
asynchronous Stage 1 connection filter before spawning the accepted connection.
A Redis-backed connection-rate-limit chain can wait for one or two bounded
dependency decisions, serializing later accepts on that file descriptor and
allowing the kernel backlog to fill. Resolve this through a separately reviewed,
plan-first bounded admission design that preserves pre-PROXY and pre-TLS security
ordering; existing integration-test files remain frozen.
