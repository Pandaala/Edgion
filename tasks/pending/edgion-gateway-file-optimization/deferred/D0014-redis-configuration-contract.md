# D0014 — Reconcile the Redis Configuration Contract

## Meta
- Status: needs-discussion
- Severity: medium
- Discovered in: B0052, Redis resource mapping and schema
- Category: architecture

## Evidence
Retry settings, `pool.minIdle`, cluster replica/redirect controls, and observability options are not consumed. Simultaneous read/write timeouts collapse to one value, the documented database range differs from runtime acceptance, and zero pool size reaches later runtime rejection.

## Why deferred
Each field needs an explicit implement-or-remove product decision; retry behavior must also address replay safety for non-idempotent commands.

## Candidate next step
Inventory every Redis field against Controller validation and Fred/runtime behavior, decide exact semantics or removal, and align schema, documentation, validation, and tests atomically.
