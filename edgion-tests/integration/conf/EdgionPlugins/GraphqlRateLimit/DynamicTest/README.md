# GraphQL cost limiting hot reload

The shared Controller/Gateway topology and existing Redis fixture are required.
The scenario uses the existing EdgionPlugins listener and test-http backend.
It starts no processes, performs no Redis flush, and uses a fresh UUID identity
for every convergence probe so retries cannot exhaust a phase budget.

Phases: invalid SDL fails closed; valid v1 charges 12 cost units; invalid reload
fails closed; repaired v2 charges 16 units for the same query. Every phase checks
the Controller Accepted condition and Gateway traffic. Updates use wait_phase.

Run through the targeted GraphqlRateLimit integration runner; its static suite
is followed by GraphqlRateLimitDynamic before final residue validation.
