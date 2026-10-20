# Evaluate prepared WAF finding metadata

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Inspected baseline: `203595a82`.

## Evidence and tradeoff

In `edgion-waf/src/seclang/transaction.rs`, execute_phase_with_prefix calls
finding_metadata only after a unit matches and while the finding is retained
under max_findings. The helper traverses the unit's compiled members and action
ranges. It keeps the last local Severity operand and counts every literal Tag
action, in compiled order. These inputs are immutable for the generation.

Evaluate preparing this pair at compilation and storing it with the compiled
unit or another bounded canonical representation. This would remove the scan
for retained findings. No-match units and findings omitted at the cap currently
pay no metadata scan, so the benefit depends on rule shape and match frequency.
Do not replace the scan with a per-request cache or another Gateway-owned index.

CompiledRuleUnit in compiler.rs currently stores phase, disruption, status,
logging and member/schedule identity. publish_with_work charges its records via
retain_compiler_records::<CompiledRuleUnit>(active_units). Extra metadata can
change its layout, compiled retained/peak cost, and acceptance near configured
memory limits. Measure the actual candidate size and padding; no exact increase
has been established. An auxiliary table has its own capacity and work charges.
The user decision is whether the measured request-path benefit justifies the
compiled representation cost, or whether to keep the current hit-only scan.

## Behavior and ownership boundaries

- Preserve last severity and literal tag count across every member of a chained
  unit. Do not reduce metadata to root actions, unique tags, reached candidates,
  or only the actions which happened to mutate TX.
- RuleFinding exposes bounded safe metadata. Keep tag text, Msg and LogData
  contents out of finding diagnostics and Gateway logs. No new logging, tracing,
  high-cardinality metric or external schema is required.
- Preserve max_findings, omitted_findings, omitted_loggable_findings, intervention
  and schedule progression. A capped result must still execute applicable actions
  and denial; skipping metadata is not permission to skip the unit.
- Preserve compiler memory/work budgets and the final publication lifecycle.
  Moving checked arithmetic/range traversal to compilation must account for its
  error-stage implications; this is not evidence of a reachable malformed private
  generation or an arithmetic defect in the current implementation.
- Keep first-member TX loans, observer restoration, post-hit staged clones,
  nonfinal breadth-first chain ownership, final-member winner order and commit
  validation. Metadata preparation does not authorize removing rollback state.

The existing contract is documented in
`skills/04-review/waf/regression-guards/fused-json-inspection.md` and
`skills/04-review/waf/regression-guards/request-scoped-control-actions.md`.

## Acceptance for later implementation

Compare empty and no-hit schedules, ordinary and chained hits, absent/present
severity, source-order severity selection, duplicate tags, max_findings zero and
nonzero caps, silent logging flags, deny/detection-only outcomes, and skip/control
interactions. Keep findings_expose_only_bounded_safe_metadata_shape and the bounded
finding tests, and add exact metadata oracles for the chosen representation.

Measure compiler record size, retained/peak/work charges and exact/one-below
budget acceptance. Include generation reload and unchanged immutable old captures.
Measure retained-hit and no-hit workloads separately, using small and action-heavy
rule units; distinguish eliminated traversal from overall matching cost.
Run leaf and Gateway WAF tests and project checks after implementation.

No runtime code, allocator/CPU measurement or new acceptance policy is delivered
by this deferred record.
