# D0080 — Reduce Per-chunk Condition Extraction Cost

## Meta

- Status: needs-investigation
- Severity: low
- Discovered in: B0121 shared condition evaluation analysis
- Category: data-plane-performance
- Canonical task: [`tasks/todo/reduce-response-body-condition-extraction-cost.md`](../../../todo/reduce-response-body-condition-extraction-cost.md)

## Evidence

Response-body conditional wrappers re-evaluate conditions for every upstream chunk. Some KeyGet, JWT, and mTLS condition sources currently materialize owned strings, parse JSON, or clone certificate information during each evaluation. The cost can therefore repeat with chunk count even when source state is unchanged.

## Why deferred

Caching a whole condition result is not behavior-preserving by default: time and probability predicates must remain dynamic, and request context values may change between plugin stages. A safe design must distinguish borrowable extraction, stable stage inputs, and intentionally dynamic predicates. The existing response-body no-work-gate task addresses outer async hook/future overhead, not this extraction contract.

## Integration-test impact

This is not a confirmed integration gap. Existing integration tests remain frozen. Any future behavior-affecting caching design must decide its own new integration coverage and record a separate issue rather than editing existing cases during this campaign.
