# Evaluate shared raw and structured request-body inspection

Status: Conditional design; P2

Current fused Full JSON/XML inspection and canonical ordered replay are implemented. Consider broader shared inspection only if profiling proves repeated raw scans are material.

- Reuse canonical BodyStorage and original source chunks; add no second body cache, DOM, or complete field map.
- Specify raw/structured scheduling, one grammar pass, true EOF, admission, transport-error priority, and provisional-memory ownership.
- Preserve TX, capture, chain, rule order, and early-intervention semantics with an explicit proof; unsupported dependencies remain strict errors.
- The request verdict must precede forwarding where required. Response-body inspection remains outside product scope.
- Review a bounded design and demonstrate a workload benefit before implementation.

Start from `skills/04-review/waf/regression-guards/fused-json-inspection.md` and the HTTP-body regression guards.
