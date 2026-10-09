# GraphQL cost limiting hot reload

The shared Controller/Gateway topology and existing Redis fixture are required.
The scenario uses the existing EdgionPlugins listener and test-http backend.
It starts no processes, performs no Redis flush, and uses a fresh UUID identity
for every convergence probe so retries cannot exhaust a phase budget.

Phases: invalid SDL fails closed; valid v1 charges 12 cost units; invalid reload
fails closed; repaired v2 charges 16 units for the same query; a scalar leaf
multiplier reload fails closed; restoring v2 charges 16 units again. The leaf
fixture keeps the query valid so ineffective-rule acceptance cannot pass unnoticed.
Every phase checks
the Controller Accepted condition and Gateway traffic. Updates use wait_phase.

Run through the targeted GraphqlRateLimit integration runner; its static suite
is followed by GraphqlRateLimitDynamic before final residue validation.

Standard SDL metadata phases accept deprecated fields and custom-scalar specifiedBy, reject null directive arguments with Accepted=False/fixed500, then recover without changing cost/debit semantics.

Root-quota phases publish a valid users budget (cost15, remaining15), reject a
non-root SDL coordinate with Accepted=False/fixed500, then recover the same named
budget. Total remains mandatory; responseQuota selects the root result.
