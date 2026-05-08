# Non-Memory-Leak Scenarios

## Stale Entries After Lazy Creation in Auxiliary LB Stores

Applicable locations:

- `src/core/gateway/backends/discovery/endpoint_slice/`
- `src/core/gateway/backends/discovery/endpoint/`

Verdict:

For the four LB store types `RoundRobin`, `ConsistentHash`, `LeastConn`, `EWMA`, an LB instance for an alternative algorithm that a service used historically may continue to remain in the corresponding store even after the route's LB policy has changed. Under Edgion's current design this phenomenon is not regarded as a memory leak.

Brief reasons:

- These instances are always strongly referenced by the global store, qualifying as a reachable runtime cache rather than objects that have lost references but cannot be released
- The early project design originally pre-built all LB algorithms for each service; the later change to on-demand creation only optimizes creation timing and does not change the per-service capacity upper bound
- The number of such instances per service has a fixed upper bound — at most one for each of 4 algorithms — and will not grow unboundedly with repeated policy switches
- When a service/endpoint's data truly disappears, the related LB entries are deleted along with the missing shared data layer

Review handling recommendations:

- Do not mark this scenario as "memory leak" or "high-severity leak"
- If pointing it out, phrase it as "bounded stale instances retained after lazy creation" or "optional optimization"
- Only when the design goal explicitly becomes "retain only algorithm instances still referenced by routes" should this be discussed further as a behavior deviation
