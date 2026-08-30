# Edgion Gateway File-by-file Optimization

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-30 |
| Status | in-progress |
| Type | refactor |
| Priority | P1 |
| Baseline commit | `d484e580` |
| Scope | Every tracked file under `edgion-gateway/` |

## AI guide summary

Audit `edgion-gateway` file by file in cohesive, dependency-aware batches. Make only evidence-backed, behavior-preserving improvements to local structure, duplication, type and error modeling, and bounded hot-path efficiency. Every implementation batch is independently reviewed, verified, recorded in one changelog, and committed locally without pushing.

## Requirements

- Cover every tracked file under `edgion-gateway/`; a reviewed file may legitimately need no change.
- Preserve the Controller/Gateway split, conf-sync semantics, `EdgionHttpContext`, access-log architecture, Pingora lifecycle, and existing public behavior.
- Prefer small local extractions and existing shared mechanisms over new frameworks or broad redesigns.
- Use an implementation subagent for each batch and a separate review subagent after implementation.
- Do not use the interactive `fix-issue-cn` workflow.
- Do not modify or delete existing integration-test files. New integration-test coverage may be added only when a behavior change genuinely requires it.
- Record every completed batch in `07-changelog.md`, then create a local commit. Never push.

## Guardrails

1. Read every in-scope file completely and inspect its direct callers, callees, tests, relevant architecture documents, and applicable closed-review precedents before editing.
2. Refactor only when the result has a concrete benefit: shorter decision flow, removed real duplication, clearer types/errors, fewer avoidable hot-path operations, or stronger local invariants.
3. Do not introduce compatibility shims, speculative abstractions, new dependencies, schema changes, metrics, or tracing merely for cleanup.
4. Preserve bounded memory and streaming/backpressure invariants. Never add response aggregation or per-chunk allocation/logging.
5. Preserve failure-policy, redaction, diagnostic, status, retry, TLS, SSRF, and configuration hot-reload semantics.
6. Do not work on the unsupported Gateway rustls backend.
7. Existing tests are a coverage floor. Unit tests may be added or strengthened; existing integration tests are immutable.
8. Before each code commit, scan the diff against every applicable red rule in `skills/00-design-rules.md`, run targeted tests, apply formatting, and run the project-required compile/lint/guard checks.
9. A reviewer must reject changes that reduce generality, obscure ownership, widen scope, duplicate a canonical mechanism, or merely move code without measurable clarity.
10. The primary agent owns final diff inspection, validation, inventory status, changelog updates, and the commit.

## Workflow per batch

1. Select the next pending cohesive batch from `file-inventory.md`.
2. Load the smallest relevant Skills subtree and binding review precedents.
3. Give the implementation subagent exact scope, invariants, forbidden changes, and verification expectations.
4. Inspect the implementation locally and run targeted verification.
5. Give a different review subagent the diff plus architecture and regression checklist.
6. Resolve every review finding and re-run relevant verification.
7. Update `file-inventory.md` and append a concise entry to `07-changelog.md`.
8. Verify that `edgion-tests` has no modified or deleted existing file.
9. Commit locally with an English message; do not push.

## Document index

| Document | Status | Purpose |
|----------|--------|---------|
| `00-context.md` | done | Mandatory context and validation references |
| `file-inventory.md` | in-progress | Complete per-file coverage ledger and batch assignment |
| `07-changelog.md` | in-progress | Unified record of actual optimizations and verification |

## Completion evidence

- Every row in `file-inventory.md` is `reviewed-no-change`, `optimized`, or an explicitly justified `excluded` state.
- Every optimized batch has an independent review result, verification evidence, and local commit in `07-changelog.md`.
- No existing integration-test file was modified or deleted relative to the baseline.
- The final repository-wide required checks pass.
- A final requirement-by-requirement audit finds no pending or weakly evidenced item.
