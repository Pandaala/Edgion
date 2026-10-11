---
id: ARCH-030
status: candidate
kind: architecture-debt
priority: P3
primary_partition: PART-295
independent_verification: confirmed by /root/verify_ip_union
---

# Give canonical bounded regex output amortized capacity growth

## Current ownership and evidence

CompiledRegexReplacement is the canonical output builder shared by DSL
regex_replace and AI Guard mask rules. compile parses an immutable replacement
template into literal ranges and capture indices. Dynamic DSL templates use
replace_all_bounded; literal templates take find_iter, while dollar-containing
templates use the same capture-token implementation as precompiled AI Guard rules.

append_bounded at lines 242–250 first calls checked_output_len, then calls
output.try_reserve_exact(value.len()) before each push_str. The reservation admits
the next logical prefix but requests no deliberate spare capacity. build_from_matches
invokes it for every unchanged slice and replacement; append_tokens invokes it
separately for each literal and present capture. A legal many-match input or
repeated-capture template therefore repeatedly reaches exact-capacity growth
instead of retaining amortized output capacity. The source-visible problem is the
growth policy, not regex compilation or unbounded output.

## Actual call chains and change scenario

DSL Vm::RegexReplace at plugins/http/dsl/lang/vm.rs:1012 invokes the dynamic helper
with max_string_len, maps the first crossing prefix to StringTooLong, maps
reservation failure to fixed Internal, and converts successful output to Arc<str>.
AI Guard CompiledAiGuardRules::evaluate at ai/guards/request/rules.rs:143 uses the
compiled helper with maxReplacementBytes minus other live replacement lengths;
it retains successful String replacements across later rules.

For example, replacing every one-character match with a small fixed literal, or
expanding several capture tokens per match, creates many nonempty append prefixes
while remaining within the output limit. The helper requests exact growth at
those boundaries whenever capacity is insufficient. Allocator rounding and
in-place expansion can reduce physical reallocations, so this review does not
claim one allocation per append, a measured quadratic runtime, or a throughput
number. The avoidable structural cost is that the sole shared regex output owner
omits an amortized bounded growth policy used elsewhere in this Gateway.

## Bounded improvement and alternatives

Keep checked_output_len as the first operation. When the admitted next length
exceeds current capacity, choose a geometric target such as
max(next_length, capacity.saturating_mul(2)).min(limit), request target minus
current length through try_reserve_exact, then append. Reserve nothing when
existing capacity already suffices. Keep fallible allocation and the inclusive
caller limit. Both existing consumers benefit through this private helper; no
new interface, shared cache, generic writer framework, or regex engine is needed.

link_sys/template/engine.rs:210–213 already demonstrates geometric growth clamped
to its byte budget, and the original request-template unification record documents
a separate exact-growth regression corrected there. Reuse that design locally,
not its runtime/type ownership: template rendering and regex interpolation have
different errors and policies. common/capped_writer.rs is an io::Write byte sink
with a sticky overflow flag and Vec growth; it neither returns the regex helper's
first crossing length nor explicitly bounds requested capacity. Replacing regex
construction with that sink would obscure required semantics. A full result-size
prepass can reserve once but walks regex matches/captures twice and complicates
early-limit/error behavior; it is unnecessary for this bounded change.

## Invariants and risks

Preserve regex 1.13 interpolation grammar, dollar branch classification,
numeric/named/braced references, unknown captures, malformed dollars, UTF-8,
zero-width matches and match order. No match must still return None without
constructing helper-owned output. Keep the first crossing prefix length,
checked-add overflow as usize::MAX, exact-limit acceptance, AllocationFailed and
InvariantViolation mappings, DSL Nil/Arc behavior, and AI Guard reject/order/chained
replacement and logical aggregate accounting.

Spare capacity trades bounded extra retained capacity for fewer growth requests.
The implementation review must assess AI Guard's multiple retained replacements
and old/new candidate overlap against its existing memory/admission assumptions;
a logical output cap alone is not a heap bound. Do not call requested capacity an
exact allocator/RSS bound or replace the independent aggregate ledger with the
per-string limit. Do not eagerly reserve the full caller limit on the first match.

## Future verification and original knowledge

Retain the current differential grammar, zero-width, no-match, exact/+1, repeated
amplification and overflow assertions. Add focused many-small-append
capacity-transition checks and saturated/clamped growth arithmetic checks,
preserving caller failure mappings and AI Guard aggregate/chained behavior.
Existing tests were read but not executed. No allocation benchmark or
process-memory measurement was run.

Original decisions read: skills/04-review/dsl/regression-guards/bounded-regex-replacement.md,
skills/04-review/plugin-runtime/regression-guards/request-template-policy-unification.md
and skills/01-architecture/02-gateway/request-template-runtime.md. The regex guard
requires bounded logical output and requested growth but does not mandate exact
per-prefix growth. This candidate is distinct from ARCH-011's final merged
replacement budget: it changes the canonical local output growth policy, not
semantic replacement selection or aggregate validation. Independent verification
is pending.
