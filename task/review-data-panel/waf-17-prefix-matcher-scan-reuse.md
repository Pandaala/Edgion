# Prefix matcher scan reuse

Status: deferred for user decision. No runtime change or selected design.
Source baseline: `47c287dda`.

## Evidence and scope

The production Gateway WafPlugin calls `execute_request_body_prefix` before
upstream dispatch. This is the active prefix path, distinct from the dormant
raw-stream API covered by WAF-16.

`Transaction::evaluate_prefix_member` clears the ordinary scan cache and borrowed
transformation memo, applies the member's transformations, checks and charges
prefix work, then calls `evaluate_operator` without a scan cache. Compatible
boolean `@pm` members can share a compiler matcher group, but every reached member
therefore scans that group again. Each member queries its own pattern range.
`Contains` uses a string relation and is outside this grouped-scan candidate.

Compiler group identity includes phase, target plan, transformation plan,
multi-match, capture mode and byte view. Body-independent rules do not share a
raw-body group merely because they have the same transformation profile.
Prefix admission already restricts body-dependent members to supported positive
operators and transformations; this task does not broaden that language.

The existing `OrdinaryScanCache` accepts eligible `SelectedBytes::Request`
arena ranges and keys them with a transformation profile. The separately borrowed
prefix is not such a range. Passing the ordinary cache through unchanged or using
pointer identity alone would not establish safe reuse. Scratch is shared with
other evaluator paths, so all writers and invalidation boundaries matter.

## Decision and acceptance

Compare retaining current execution against bounded, prefix-call-scoped reuse
through canonical matcher/cache ownership. Single-entry and broader retention
have different hit rates and memory costs; no choice is selected. Do not add a
second independent matcher, retain prefix bytes, or create another body store.
Measure repeated compatible groups, alternating profiles, few rules, early deny
and DetectionOnly before claiming a net benefit. No allocation, throughput or
RSS improvement is established by this source inspection.

Any future implementation must preserve:

- Lazy source-order evaluation, removal/policy skips and immediate enforced deny;
  unreachable rules must not introduce work or errors.
- Every reached rule's transformation execution, output/work checks, evaluation
  count and checked prefix operator-work charges. This is not transformation
  memoization or permission to reduce logical charges.
- Input/profile and generation identity, phase/call invalidation and every scratch
  overwrite. Publish reusable validity only after successful scanning and range
  validation; never reuse stale bits after an error or intervening writer.
- Accelerated/Reference parity, member range isolation and existing failure order.
- Pre-dispatch verdicts, prefix completion semantics, independent physical body
  limits and canonical BodyStorage ownership. No raw-stream reactivation.
- Explicit bounded ownership and actual-capacity accounting for any added state;
  no new request logging, metrics, shared mutable scratch or unsafe code.

Freeze the exact implementation allow-list and obtain independent IMPL plan and
final review before changing runtime code. Tests must cover compatible repeated
groups, different profiles, mixed body-independent rules, early deny versus
DetectionOnly, exact/exceeded budgets, failures, and cache invalidation across
all relevant writers. Existing direct-prefix invalidation tests remain required.

## Sources and verification boundary

- `edgion-gateway/src/plugins/http/waf/mod.rs`: active prefix/full dispatch.
- `edgion-waf/src/seclang/compiler.rs`: prefix eligibility and matcher group keys.
- `edgion-waf/src/seclang/transaction.rs`: prefix scheduling, ordinary cache and
  `evaluate_matcher_operand_cached`.
- [Prefix-before-dispatch contract](../../skills/04-review/http-body/regression-guards/waf-prefix-before-dispatch.md).

This records a source-level optimization candidate, not a correctness defect.
No prototype, benchmark, full suite or live integration was run for the decision.
Historical verification does not prove a future cache design.
