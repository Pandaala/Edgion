# B0256 — HTTP DSL Runtime

## Scope

- Pipeline state: analysis-in-progress; queued behind active B0255.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `53b436aa`.
- Target: `edgion-gateway/src/plugins/http/dsl/runtime.rs`.
- Target blob: `fec56ae891bd43b996b90b36657bd87a43263d08`.
- Read scope includes the complete runtime construction/execution lifecycle, plugin/VM/builtin consumers, resource inputs, tests, and directly owning architecture/review knowledge.
- The prior B0254 reviewer identified `skills/01-architecture/02-gateway/dsl-engine.md` as a possible exact architecture-support path for construction-time wording, the `rate_limits` excerpt, and the body-scanner path. The analyst must independently confirm scope and value.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Pending analyst report. This candidate cannot enter implementation/review before B0255 commits and its findings are refreshed against the resulting HEAD.
