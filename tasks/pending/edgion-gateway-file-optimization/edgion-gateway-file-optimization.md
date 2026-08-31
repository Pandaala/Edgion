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
| Campaign status | in-progress |
| Coverage | 530 tracked files: 55 changed, 70 examined-no-change, 5 deferred, 400 pending |
| Active batch | [`B0047-link-sys-elasticsearch`](batches/B0047-link-sys-elasticsearch.md) |
| Analysis queue | none |

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
- Treat `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` as protected user-owned paths.

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
10. The primary agent owns mechanical scope containment, validation command execution, inventory and changelog updates, exact staging, and the commit. Technical analysis, implementation, and review stay with separate agents.

## Workflow per batch

1. Select the next pending cohesive batch from `03-subtasks.md`, mark it in progress, and freeze its exact path boundary.
2. Run the integration freeze check, then delegate a fresh read-only analysis and value gate.
3. For a justified change, use a different implementation agent and a fresh independent reviewer; complex work also receives a separate plan review before edits.
4. Defer wider or design-heavy findings without expanding the batch.
5. Run targeted verification plus every repository-required pre-commit guard.
6. Update the batch record, ledger, changelog, and deferred index.
7. Verify the frozen paths and exact staged file set, commit locally in English, and never push.

## Document index

| Document | Status | Purpose |
|----------|--------|---------|
| `00-context.md` | done | Mandatory context and validation references |
| `03-subtasks.md` | in-progress | Complete per-file coverage ledger and batch assignment |
| `07-changelog.md` | in-progress | Unified record of actual optimizations and verification |
| `integration-freeze.tsv` | done | Content and mode fingerprints for the immutable integration-test floor |
| `batches/` | in-progress | Decision, scope, review, and verification record for each new-format batch |
| `deferred/INDEX.md` | in-progress | Backlinks to canonical wider-scope task records |

## Completion evidence

- Every row in `03-subtasks.md` has a terminal campaign status or a documented pending batch.
- Every changed batch has an independent review result, verification evidence, and local commit in `07-changelog.md`.
- No existing integration-test file was modified or deleted relative to the baseline.
- The final repository-wide required checks pass.
- A final requirement-by-requirement audit finds no pending or weakly evidenced item.
