# D0013 — Define a Strict Redis Endpoint Contract

## Meta
- Status: needs-discussion
- Severity: high
- Discovered in: B0052, `edgion-gateway/src/link_sys/providers/redis/config_mapping.rs`
- Category: correctness

## Evidence
Cluster and Sentinel mapping silently discard malformed endpoints, standalone accepts extra unused endpoints, and current validation does not define uniform URL syntax or topology-wide transport rules.

## Why deferred
A correct contract crosses resource validation, Controller status, Gateway parsing, Fred topology semantics, and secret-safe error reporting.

## Candidate next step
Define topology-specific cardinality, supported URL components, all-or-nothing parsing, and uniform active-transport validation, then enforce the same contract before publication and at Gateway construction.
