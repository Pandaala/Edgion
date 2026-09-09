# B0237 — AI Guard Rules

## Scope

- Pipeline state: changed; verification and independent review complete, awaiting local commit.
- Active anchor: `edgion-gateway`.
- Baseline: `5b050b0d`; target blob `5ffc064e2bed95afba9ae75ea2f7bcb71454fc59` unchanged through B0236.
- Target: `edgion-gateway/src/plugins/http/ai_guard/rules.rs`.
- Inspect complete rule compilation/evaluation, configuration semantics, regex ownership, callers and tests. B0236 separately owns plugin/body flow.
- Root owns Cargo and all writes. Existing integrations, external todos and handoff/WAF paths remain protected. Revalidate after B0236 before promotion.

## Analysis Outcome

- Decision: change, complex/plan-first. `regex.replace_all(...).into_owned()` currently materializes the complete amplified result before enforcing `maxReplacementBytes`; valid capture references can therefore allocate far beyond the advertised 1 MiB ceiling. Multiple live replacements can also exceed the aggregate cap before the final sum.
- Freeze replacement templates once per immutable rule generation, preserving regex 1.13 replacement grammar. Replace the duplicate `is_match` plus uncapped `replace_all` scans with one bounded construction pass that checks literals, unchanged slices and capture expansions before appending.
- Track bytes held by all live masked span values while rules chain: subtract the prior value before a rewrite and reject before the new aggregate crosses the cap. Preserve rule order, role filtering, staged-allow counts, immediate reject, later-rule visibility, sorted output and existing failure-policy routing.
- Exact code/test scope is `rules.rs`; maintained CPU/memory knowledge may be updated only if implementation evidence requires it. No cross-crate, integration, logging, metric or body-cache change is warranted.
- Required tests: numeric/named captures and `$$`; exact cap and plus one during construction; repeated-capture amplification; aggregate multi-span overflow; chained replacement accounting; and oversized no-match pass. Existing rules and full AI Guard tests remain the floor.
- Freshness: the original analyst revalidated the target, plugin call boundary and failure-policy route at `5b050b0d`. B0236 left `CompiledRules::evaluate`, borrowed `RuleSpan`, `GuardDecision` and `Err(())` handling unchanged. The plan-first finding and target-only scope remain valid.
- The implementation plan must reproduce regex 1.13 replacement grammar exactly without an uncapped expansion scratch, avoid allocating a body-sized output on no-match, and bound both candidate construction and the total live divergent replacement strings across chained rules. Any public/schema/interface expansion requires re-review.

## Outcome

- The first independent plan review rejected two inaccurate interpolation assumptions and one accounting shortcut: numeric references use `parse::<usize>()` before name fallback, malformed braced syntax emits the current dollar and resumes scanning, and an empty divergent replacement remains active even though its charged length is zero. The corrected full plan received independent approval before implementation.
- Mask replacements now compile once into private coalesced literal/capture tokens. The parser mirrors the locked regex interpolation grammar, including longest unbraced names, braced UTF-8 references, signed/leading-zero numeric parsing, overflow fallback, unknown references, `$$`, malformed dollars and missing braces.
- Mask evaluation uses one `captures_iter` traversal. Every unmatched, literal and capture slice is checked against an inclusive logical byte limit before fallible reservation and append; no replacement-output string is created before the first match, and there is no uncapped expansion fallback.
- Live divergent replacement bytes are charged across every span and chained rule. Candidate construction first removes only the current slot's charge from the available budget while retaining the old value as input, then atomically preserves, clears or replaces the slot. Retained text is at most the configured cap; simultaneous old-plus-candidate logical text is at most twice the cap.
- Sixteen rule tests cover exact upstream interpolation differentially, exact/plus-one limits, repeated-capture amplification, multi-span aggregate overflow, chained shrink/restoration, zero-cap deletion, missing replacement and oversized no-match behavior. Existing order, role, allow/reject, flagged and later-rule semantics remain covered.
- Fresh independent final reviewer `b0236_ai_guard_plugin_analysis`: LGTM, no Must Fix or Should Fix. It independently checked the locked upstream grammar, empty-match iteration, byte arithmetic/reservation, aggregate accounting, failure routing, full call chains and project hard rules.
- Verification: rules 16/16 and full AI Guard 40/40; workspace format/check/Clippy (existing warnings), agent docs, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff and 3,136-file freeze passed. No integration, schema, logging, metric, documentation or public interface edit.
- Local commit SHA is backfilled by the next batch; never push.
