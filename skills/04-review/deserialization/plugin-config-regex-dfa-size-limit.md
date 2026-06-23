---
name: plugin-config-regex-dfa-size-limit
description: Use when reviewing plugin config structs (proxy_rewrite / ctx_set / direct_endpoint / dynamic_*_upstream / cors / request_restriction) that compile an operator-supplied regex with bare `Regex::new` and a finding proposes capping the DFA via `RegexBuilder` size_limit/dfa_size_limit; explains why this is an accepted tradeoff (not fixed) despite the DSL precedent.
status: accepted-tradeoff
finding_id: fr-hplug-01
closed: 2026-05-31
---

# Plugin config `Regex::new` without DFA size_limit — ACCEPTED-TRADEOFF (not fixed)

## Conclusion

The plugin config structs compile operator-supplied regex patterns with bare
`regex::Regex::new(pattern)` (no `size_limit` / `dfa_size_limit`) at the following sites:

- `edgion-resources/src/resources/edgion_plugins/plugin_configs/proxy_rewrite.rs` (`check_fields`, `precompile`)
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/ctx_set.rs` (`validate` — extract + transform)
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/direct_endpoint.rs` (`validate` — extract)
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/dynamic_external_upstream.rs` (`check_fields`, `validate`)
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/dynamic_internal_upstream.rs` (`validate`)
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/cors.rs`, `request_restriction.rs` (same pattern, adjacent)

Proposals to replace these with the P3-g5-015 capped builder
(`RegexBuilder::new(pattern).size_limit(1 << 20).dfa_size_limit(1 << 20).build()`) are
**not accepted as a code change** at this time. This is a documented, accepted tradeoff —
NOT a confirmed bug to fix.

## Core Rationale

**1. This is not ReDoS — Rust's `regex` is linear-time by construction**

The original finding (and any re-report) frames this as a ReDoS / "pathological backtracking"
risk and cites patterns like `(a+)+b`. That framing is incorrect for this codebase: the Rust
`regex` crate uses finite automata (NFA / lazy DFA), so matching is **always linear in input
size** regardless of pattern — there is no catastrophic backtracking to exploit. The compiled
regex is cached in the plugin struct; per-request `captures()` is linear. There is no
per-request attack surface here at all.

**2. The only real axis is compile-time DFA memory, and it is already bounded**

The actual (narrow) concern is the memory the regex compiler allocates while *building* the
automaton. But `regex` already defaults `size_limit` to 10 MB per DFA — this is a bounded
allocation, not an unbounded one. The proposed fix only tightens 10 MB → 1 MB. The "20 plugins
× 5 rules × 20 MB ≈ 2 GB" amplification in the finding requires a trusted operator to
deliberately author many large-DFA patterns across many plugin instances — a self-inflicted
DoS by an already-trusted actor.

**3. Threat model requires operator-level CRD write — marginal benefit**

Reaching these code paths requires write access to `EdgionPlugin` / `HTTPRoute` CRDs (the
config-loading / hot-reload path, ConfHandler — not the data-plane hot path). An actor who can
write CRDs can already repoint traffic, ship arbitrary configs, and disrupt the gateway in far
more direct ways. Capping their regex DFA from 10 MB to 1 MB is a very marginal defense-in-depth
increment against an actor already inside the trust boundary, or against an operator's own
mistake. The primary security boundary remains K8s RBAC over the plugin CRDs.

**4. Tension with the DSL precedent (P3-g5-015) is acknowledged, not overriding**

P3-g5-015 capped the three DSL pipeline sites at 1 MB and its closed-finding note generalised
the rule to "future plugin config that accepts user-written regex"
(`skills/04-review/dsl-regex/dfa-size-limit.md`). The DSL cap is cheap and already shipped, so
it stays. Extending it to these config structs is *consistent* but not *required*: per the
reasoning above the security value is marginal, so the inconsistency is accepted rather than
closed with a mechanical multi-file change. If the cap is ever applied here, do it as a low-risk
consistency cleanup bundled with other work in these files — not as a security fix.

## Fix Suggestions Not Accepted

- "Replace `Regex::new` with `RegexBuilder…size_limit(1<<20).dfa_size_limit(1<<20)` at the six/eight
  sites" — correct mechanically, but the security justification (ReDoS / unbounded allocation) is
  wrong, and the marginal benefit under an operator-trusted threat model does not justify a code
  change on its own.
- "This is a ReDoS vulnerability" — false; Rust `regex` does not backtrack, matching is linear.
- "Unbounded DFA allocation" — false; the default 10 MB `size_limit` already bounds it.

## Re-evaluation Triggers

Re-open (i.e. apply the 1 MB cap as a code fix) only if:

- These files are being edited for another reason anyway and the cap can ride along at near-zero
  marginal cost / risk (consistency cleanup, not a security fix).
- The threat model changes so that regex patterns can reach these compilation paths *without*
  operator-level CRD write access (e.g. a new lower-privilege config surface feeds them).
- A production incident shows operator-authored or accidental large-DFA patterns actually
  spiking gateway memory during hot-reload.

## Reference Cases

- fr-hplug-01 (2026-05-31) — this finding; closed as accepted-tradeoff at the user's decision.
- P3-g5-015 (2026-05-16) — DSL regex DFA cap (the cheaper, shipped precedent):
  `skills/04-review/dsl-regex/dfa-size-limit.md`.
- Generalised "compile regex from operator input" rule:
  `skills/04-review/deserialization/SKILL.md` §"Regex recompilation on deserialize".
