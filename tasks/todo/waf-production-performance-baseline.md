# Measure representative Gateway WAF performance

Status: Pending measurement; P2

Collect release Gateway end-to-end performance evidence after the completed WAF optimizations. Historical leaf allocation measurements do not establish production latency, throughput, or RSS.

- Compare standard/strict, On/DetectionOnly, and expensiveChecks off/on on the same host with fixed source pins and build features.
- Include realistic clean and attack traffic, JSON/XML/form/multipart, body sizes, concurrency, and a no-WAF baseline.
- Report throughput, p50/p95/p99, peak RSS, and attributed allocation/CPU costs.
- Separate immutable compile/reload memory, request scratch, BodyStorage, conservative admission forecasts, and observed process memory; shared per-rule costs are not additive.
- Open targeted optimization tasks only for attributed remaining costs. Existing JSON capture keys already avoid per-key heap allocation.

Reuse `edgion-waf/examples/waf_rule_memory_costs.rs`, existing benchmarks, and the integration Controller/Gateway topology. Add no production request logging for measurement.
