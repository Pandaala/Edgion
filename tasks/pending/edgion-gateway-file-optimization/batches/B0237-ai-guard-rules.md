# B0237 — AI Guard Rules

## Scope

- Pipeline state: analysis-ready; queued for plan-first promotion after B0236 commits, no mutation authorized.
- Active anchor: `edgion-gateway`.
- Baseline: `6a5e428a`; target blob `5ffc064e2bed95afba9ae75ea2f7bcb71454fc59` unchanged through B0236 implementation.
- Target: `edgion-gateway/src/plugins/http/ai_guard/rules.rs`.
- Inspect complete rule compilation/evaluation, configuration semantics, regex ownership, callers and tests. B0236 separately owns plugin/body flow.
- Root owns Cargo and all writes. Existing integrations, external todos and handoff/WAF paths remain protected. Revalidate after B0236 before promotion.

## Analysis Outcome

- Decision: change, complex/plan-first. `regex.replace_all(...).into_owned()` currently materializes the complete amplified result before enforcing `maxReplacementBytes`; valid capture references can therefore allocate far beyond the advertised 1 MiB ceiling. Multiple live replacements can also exceed the aggregate cap before the final sum.
- Freeze replacement templates once per immutable rule generation, preserving regex 1.13 replacement grammar. Replace the duplicate `is_match` plus uncapped `replace_all` scans with one bounded construction pass that checks literals, unchanged slices and capture expansions before appending.
- Track bytes held by all live masked span values while rules chain: subtract the prior value before a rewrite and reject before the new aggregate crosses the cap. Preserve rule order, role filtering, staged-allow counts, immediate reject, later-rule visibility, sorted output and existing failure-policy routing.
- Exact code/test scope is `rules.rs`; maintained CPU/memory knowledge may be updated only if implementation evidence requires it. No cross-crate, integration, logging, metric or body-cache change is warranted.
- Required tests: numeric/named captures and `$$`; exact cap and plus one during construction; repeated-capture amplification; aggregate multi-span overflow; chained replacement accounting; and oversized no-match pass. Existing rules and full AI Guard tests remain the floor.
