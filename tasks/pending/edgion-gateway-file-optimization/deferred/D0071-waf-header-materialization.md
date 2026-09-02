# D0071 — Reduce WAF Header Materialization

## Meta
- Status: needs-investigation
- Severity: medium
- Discovered in: B0106, WAF HTTP plugin analysis
- Category: performance-architecture
- Canonical task: [`tasks/todo/reduce-waf-header-materialization.md`](../../../todo/reduce-waf-header-materialization.md)

## Evidence
The current WAF path converts every request and response header name/value into temporary owned vectors through `PluginSession`, then the `edgion-waf` transaction copies those bytes again into its own contiguous storage. Header-heavy WAF traffic therefore pays per-field allocation and duplicate copying before rule evaluation.

## Why deferred
A safe fix requires a benchmark-backed, borrow-scoped object-safe header visitor or a transaction API that safely consumes owned header storage. It crosses the session trait, Pingora adapter, WAF plugin, `edgion-waf` transaction input, and mocks. That architecture work is wider than B0106's validation and local request-head optimization.

## Integration-test impact
Existing WAF integration behavior should remain unchanged. Add deterministic allocation/throughput evidence and crate-local API tests first; create a separate integration issue only if the eventual design changes observable lifecycle behavior.
