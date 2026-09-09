# B0253 — HTTP DSL VM

## Scope

- Pipeline state: in-progress; complex change, plan-first.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `9d3d0354`.
- Target: `edgion-gateway/src/plugins/http/dsl/lang/vm.rs`.
- Target blob: `eb43162bdc36687bc603feeba92997cc2d1f8db1`.
- Read scope includes the complete DSL compiler/validator/runtime, request-body storage, AI Guard bounded replacement precedent, plugin/error-policy callers, and tests.
- Proposed technical allow-list: target; `edgion-gateway/src/plugins/http/common/mod.rs`; new canonical `edgion-gateway/src/plugins/http/common/bounded_regex_replacement.rs`; `edgion-gateway/src/plugins/http/ai_guard/rules.rs`; `edgion-gateway/src/plugins/http/dsl/tests/utility_functions.rs`; DSL security/review knowledge. The implementation plan must freeze the exact list before edits.
- Existing `edgion-tests/**`, coordinator records, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected from technical agents.

## Analysis Outcome

- Decision: change; complex/plan-first.
- `RegexReplace` calls `regex::Regex::replace_all` before enforcing `maxStringLen`. Legal dynamic replacements with repeated capture references can allocate an output vastly above the sandbox limit before `StringTooLong`, creating a request-path availability defect.
- B0237 already implemented and differentially tested regex 1.13 interpolation grammar for bounded AI Guard replacements. Copying it into DSL would violate cross-cutting unification; promote it into one HTTP-plugin common helper, mechanically retain AI Guard semantics, and use it in DSL.
- Required invariants: checked bounded construction, no allocation above the admitted result cap, numeric/named/braced captures, longest unbraced references, `$$`, malformed/unknown references, UTF-8 capture names and zero-width matches remain regex-compatible; no-match retains the DSL `Arc` clone fast path; overflow maps to existing `StringTooLong`; impossible helper invariants disclose no input; error-policy and logging behavior remain unchanged.
- Fresh complete DSL unit filter passed 319 tests at analysis time. No integration edit is required.

## Approved Plan

- Freeze exactly eight technical paths: the VM, common module plus new bounded-replacement helper, AI Guard rules, DSL utility tests, DSL security model/review index, and one new regression guard. Cargo, sandbox user docs, integrations, and coordinator records remain excluded.
- Provide one crate-private helper with compiled and dynamic entry points. Templates containing no `$` retain regex 1.13's `find_iter` literal fast path; every `$`-containing template uses the compatible capture grammar and `captures_iter`. Dynamic no-match avoids helper output/token construction and preserves the VM's subject `Arc` clone.
- Bound every helper-owned output append with checked logical prefix length before requesting capacity. Report the first crossing prefix as `StringTooLong`, use `usize::MAX` on checked-length overflow, and map payload-free reservation/invariant failures to fixed internal errors. Do not claim control of regex scratch space, allocator rounding, token metadata, or the later `Arc` conversion.
- Mechanically migrate AI Guard while preserving snapshot compilation, ordering, roles, flags, restoration, aggregate live replacement accounting, failure policy, and diagnostics. Add differential regex-grammar/zero-width tests, explicit literal-versus-capture branch locks, exact/plus-one/amplification limits, and direct DSL error mapping.
- Independent plan review initially rejected unconditional capture extraction and absolute allocation claims. The revised plan restored the no-`$` `find_iter` branch, narrowed claims, and added branch/error tests; re-review approved with no remaining Must Fix.

## Separate Candidates

- `BodyRead` and `BodyReadAll` allocate raw/lossy output before the final string limit. This is a distinct request-body-storage change and must not expand the regex batch; independent final review must confirm its standalone task scope.
- `sandbox-security.md` contains stale panic-detail and `http.call` streaming-failure descriptions. This is documentation-only and must not alter runtime observability or failure policy; independent final review must confirm disposition.

## Implementation and Review

- Added one crate-private bounded replacement helper with compiled and dynamic entry points. Literal templates use `find_iter`; capture templates use regex-compatible token expansion and `captures_iter`.
- DSL now checks every admitted output prefix before allocation, reports the first crossing prefix through `StringTooLong`, retains the no-match `Arc` clone, and uses fixed payload-free internal failures.
- AI Guard now consumes the same helper while preserving snapshot compilation, rule order, roles, flags, restoration, chained replacement behavior, and aggregate live replacement accounting.
- Independent final review: LGTM, no Must Fix or in-scope Should Fix. The reviewer checked regex 1.13 interpolation and zero-width behavior, exact/plus-one limits, helper-owned allocation claims, DSL error/clone behavior, AI Guard accounting, observability, and scope.
- Independently confirmed separate tasks: D0129 for bounded DSL body reads and D0130 for sandbox security documentation accuracy. Neither expands this batch.

## Verification

- Implementer: common helper 7/7; AI Guard rules 15/15; full AI Guard 48/48; DSL utility functions 54/54; full DSL 323/323; Gateway all-target compile; format check; diff check; agent docs; English-only; Gateway no-tracing.
- Coordinator: `cargo fmt --all`; locked workspace check; locked workspace Clippy (exit 0 with pre-existing warnings); agent docs; SSA force; metrics inventory; Pingora source policy; Gateway no-tracing; English-only; diff check; and frozen paths all passed.
- Frozen integration manifest: 3,231 paths unchanged.
