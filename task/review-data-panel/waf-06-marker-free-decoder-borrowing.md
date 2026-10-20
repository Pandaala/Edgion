# WAF marker-free decoder borrowing

Status: deferred workload and physical-accounting decision for the user.
Source baseline: `a75784ab2`. No runtime or dependency change.

## Evidence

Gateway WAF rule execution reaches the shared walk_transformations implementation
in edgion-waf/src/seclang/transform.rs. Its identity cases currently cover None,
unchanged Lowercase, marker-free RemoveNulls, HtmlEntityDecode and UrlDecodeUni.
CssDecode, EscapeSeqDecode and JsDecode copy every byte unchanged when their
input contains no backslash, but still enter the materializing scratch path.

A shared marker-free check could retain the current input or previous scratch
buffer instead of clearing, reserving and populating the destination. This can
avoid a copy and fresh capacity, but does not release scratch already retained
from earlier steps. Marker-bearing inputs would pay an additional scan before
normal decoding. Existing identity cases are scoped precedent, not performance
measurements for these three decoders or all recipe/input distributions.

The physical consequence is observable: ProfileObserver in
json_inspection_collector.rs forwards admit_scratch and scratch_capacity to the
inspection store in json_inspection.rs. These operations enforce peak admission
and update retained usage. Avoiding a destination can change near-budget failure
and later headroom even when output bytes, logical work and checkpoints match.
This is not evidence that current accounting is incorrect.

In ordinary rule evaluation, evaluate_single_input_with_transform_memo in
transaction.rs records successful Cow::Borrowed results in the existing
BorrowedTransformMemo. Extending identity recognition can expand memo eligibility
as well as avoid the immediate copy. No additional cache is needed or proposed.

## Decision

Keep current materialization, or extend the canonical identity recognition for
exactly these three decoders after accepting the physical-admission consequences
and evaluating representative recipes. Compare marker-free and marker-bearing
inputs, short and long values, fresh and retained scratch, ordinary and multiMatch
paths, and repeated eligible inputs. Measure copy/capacity and elapsed execution
separately. Do not claim every unchanged value avoids an allocation or that an
extra prescan is free.

## Acceptance for an approved implementation

Preserve initial and per-step output caps, local work limits, checkpoint order,
changed-stage ordinals, multiMatch output and validation of every later step.
Keep identity eligibility conditional on the same output-cap policy as existing
marker-free cases. Do not return early from the recipe after proving one step.

Review actual scratch lifetime, peak admission and retained headroom explicitly;
do not silently invent virtual physical charges or suppress real capacity charges.
Test exact limits, one-below limits, nonempty retained buffers, failure after an
identity step and switching between input and both scratch buffers. Preserve
physical-error propagation and the Gateway failure policy.

Verify BorrowedTransformMemo source range/profile/local-limit identity, owning
lifetime, clearing before failed validation and downstream charging. Identity
proof must still cover the complete recipe, not just one decoder. Keep existing
memo scope and do not add a second body cache or per-decoder observer machinery.

The anchor remains edgion-gateway. A future shared-leaf implementation requires
an exact supporting-file allowlist and IMPL plan/final reviews. Keep integration
fixtures unchanged and add no request tracing, payload logging or metrics.

## Knowledge and verification limits

See [fused JSON inspection](../../skills/04-review/waf/regression-guards/fused-json-inspection.md)
for existing identity and scratch boundaries. Current tests establish the current
implementation only. This task implements no new identity cases and contains no
benchmark or proof of changed budget acceptance.
