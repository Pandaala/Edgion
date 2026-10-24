# Prepared WAF template shape

Status: deferred for user decision. No runtime representation selected.
Source baseline: `86ece629a`.

## Evidence

`CompiledTemplate` currently stores four usize fields: segment start/count,
static byte count and runtime byte limit. `compile_segments` already distinguishes
literal and macro segments, but does not retain a template-level shape flag or
prepared literal range.

At runtime, `static_template_operand` validates the template/segment interval and
scans for a macro. If any macro exists it returns None, causing canonical owned
expansion. Otherwise it walks literal segments again to validate byte ranges,
contiguity and each cumulative length against the effective member/template limit,
then borrows the combined generation range. Current consumers include string
operators, templated integer operands and TX deletion keys.

Static byte count alone does not establish absence of macros: a dynamic template
may have no literal bytes or expand a missing value to empty. This is a repeated
classification candidate, not a request-time compiler or correctness defect.

## Decision alternatives

- Retain current scans if templates are usually short and metadata/layout cost
  outweighs avoided traversal.
- Evaluate a cold shape flag that only avoids the macro-presence scan.
- Separately evaluate a prepared literal descriptor if it can preserve all
  required range and per-segment validation behavior without duplicating the
  template interpreter or expanding generation storage disproportionately.

No alternative is chosen. Measure real segment counts, static/dynamic mix and
repeated operand evaluation, not only a long synthetic template. This is distinct
from WAF-19's setter/key ownership decision; no write-key API change, runtime
cache or new MatchValue variant is authorized here.

## Representation and semantic gates

Record actual CompiledTemplate layout and total generation cost for each option.
Its size contributes to preflight known-owned estimates and per-record compiler
owned/peak admission. Reconcile every applicable retained/working allocation
charge and near-limit outcome, including Controller/Resources/Gateway compilation
parity. Do not treat spare padding or metadata storage as free without evidence.

Preserve template/segment bounds, zero-segment empty identity, literal-range
validation and contiguity, effective member limits, and the first failing
cumulative prefix's error value. Replacing per-segment checks with one final
length check can change the reported actual length. Current macro detection also
precedes all literal validation: fusing those scans can change competing errors
for invalid private generation state. Any changed invariant boundary requires
explicit design review rather than silently relying on current happy-path output.

Dynamic templates must still resolve and meter macros in order, including missing
values and sealed structured-result reads. Validation-only expansion must keep
those checks without retaining discarded output. Preserve action scheduling,
failure policy, sensitive-data redaction and current allocation ownership.

Before implementation, freeze an allow-list and obtain independent IMPL plan and
final review. Retain `delete_key_templates_borrow_static_bytes_and_preserve_expansion_limits`
and `borrowed_static_operand_preserves_first_prefix_error_and_empty_identity`.
Add chosen-representation tests for compiler admission, empty/multiple literals,
mixed macros, malformed ranges, differing member limits and exact/one-less bounds.
Compare allocation/metadata and end-to-end execution costs before claiming a gain.

## Sources and current verification

- `edgion-waf/src/seclang/compiler.rs`: CompiledTemplate, compile_segments,
  known_owned_bytes and push_record_charge.
- `edgion-waf/src/seclang/transaction.rs`: static_template_operand,
  template_operand, expand_template and operator/action consumers.
- [Canonical compiler and structured-result boundaries](../../skills/04-review/waf/regression-guards/fused-json-inspection.md).

Fresh unchanged-source checks passed 17 unique leaf tests selected by template,
setvar and the prefix-error regression, plus 52 Gateway WAF tests. Formatting and
workspace all-target check/Clippy passed with existing warnings. These checks do
not validate an alternative layout. No prototype, benchmark, full suite or live
integration ran; no throughput, allocation or RSS benefit is established.
