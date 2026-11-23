---
name: ordinary-rule-work-budget-scope
description: Review the accepted scope of ordinary SecLang work accounting across rule units.
knowledge_type: intentional-design
---

# Ordinary rule work is limited per rule unit

For `waf-02`, the project accepted cumulative ordinary work across distinct
rule units and chose to keep the existing per-unit budget. A new aggregate
cutoff would also stop legitimate inspections that fit every per-unit limit.
There is no calibrated transaction threshold or measured Gateway latency
impact for the workload described in the finding.

The [chain accounting guard](../regression-guards/chain-ordinary-work-accumulates-per-unit.md)
describes the counter, its reset and the regression tests. In full request-body
inspection the Gateway calls `Transaction::execute_request_body` synchronously.
Expensive inspections can therefore delay other requests on the same worker.
Work bytes approximate cost and do not measure CPU time. This decision covers
ordinary rule evaluation only; the [JSON override ceilings question](../../../../tasks/improve-issues/waf-json-inspection-budget-ceilings.md)
remains open.

Revisit if representative Gateway traffic shows material tail latency for
other requests attributable to synchronous WAF inspection, or if rule
scheduling or accounting changes invalidate the per-unit bound.
