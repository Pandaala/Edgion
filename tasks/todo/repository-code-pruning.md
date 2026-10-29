# Repository Code Pruning, Test and Comment Simplification

## Meta

| Key | Value |
| --- | --- |
| Created | 2026-10-28 |
| Status | pending |
| Type | refactor |
| Current phase | Specification only; execution has not started |

## Objective

Review all repository-owned code after the accumulated refactors. Remove proven
unused code and simplify tests so the repository is smaller and easier to
maintain while preserving supported behavior and meaningful regression coverage.
Simplify stale, redundant, and visually disruptive comments as part of the review.
There is no deletion quota or target percentage of tests to remove.

## Scope

- All ten workspace crates, including production code, unit tests, integration
  harnesses, test helpers, benchmarks, and build scripts.
- Repository-owned automation and executable examples outside the workspace,
  including CI, deployment scripts, and test runners.
- Manifests, fixtures, configurations, schemas, and documentation when needed to
  establish reachability or remove references to deleted code.
- Inventory every tracked path. Classify non-code, generated, vendored, and
  historical artifacts explicitly; inspect their consumers where relevant.
  Generated and third-party contents are not manually rewritten.
- Preserve pre-existing user changes and untracked artifacts, including the
  existing untracked `impl/` directory. Do not count them as reviewed or delete
  them as part of this campaign.

New features, broad architecture redesign, unrelated cosmetic rewrites, unsupported
Gateway rustls implementation, and unrelated backlog cleanup are out of scope.
Protected dormant seams are retained unless their removal is separately authorized.

## Functional Preservation

- Preserve existing supported features, defaults, error semantics, configuration
  and wire contracts, lifecycle behavior, and hot-path resource bounds.
- Establish relevant behavioral baselines before deleting code or simplifying
  tests. A passing compile alone does not establish functional equivalence.
- If a proposed removal changes working behavior, retain it and record the
  candidate for separate consideration. This cleanup does not authorize feature
  removal or behavior redesign, even when the existing behavior looks redundant.

## Code Deletion Rules

1. Require concrete evidence that a symbol, branch, module, dependency, script,
   or fixture is unused or belongs exclusively to a removed implementation.
2. Check callers across crates, trait dispatch, macros, registries, configuration
   and serialization entry points, build scripts, CI, and supported feature and
   platform combinations. Public visibility, absent text matches, compiler
   warnings, and zero runtime coverage are clues rather than proof.
3. Do not delete a production helper merely because only tests currently call it
   without investigating the supported contract and intended entry points.
4. Remove exclusively owned tests, helpers, dependencies, registrations, and stale
   references together. Preserve shared consumers and external protocol contracts.
5. Classify uncertain candidates as deferred, with evidence gaps and a concrete
   revisit condition. Do not invent deletion candidates just to change every file.

## Test Simplification Rules

- Retain tests for supported behavior, critical boundaries, meaningful failures,
  and historical regressions. Follow `skills/05-testing/00-unit-testing.md`.
- Remove tests of deleted code and tests with no meaningful assertion or no
  distinct contract, after checking whether they hide a missing useful assertion.
- Consolidate duplicate setup, fixtures, and equivalent ordinary cases. Prefer
  small local helpers or named case tables over a new generic test framework.
- Preserve protected unit coverage: algorithms, data-structure invariants,
  security and cryptography, error text, protocol/serialization contracts,
  supported version conversions, and bug-ID regressions. Retain individually
  diagnosable protected tests; integration coverage alone does not replace them.
- For each deleted or merged test, record its old identity and the exact surviving
  assertions/cases, or identify the removed production behavior it exclusively
  tested. Similar names or a shared success status do not establish equivalence.
- Keep integration coverage of runtime wiring, configuration synchronization,
  hot reload, and lifecycle behavior. Integration tests may be simplified only
  with equivalent scenario and assertion evidence; they are not globally frozen.
- Do not silence failures, skip tests, weaken assertions, reduce boundary inputs,
  or bulk-refresh snapshots to obtain a smaller or passing suite. Preserve test
  discovery, feature gates, private access, and the required test-file layout.

## Comment Simplification Rules

- Remove obsolete refactor narratives, commented-out code, duplicate explanations,
  and comments that merely restate the next statement.
- Shorten oversized banners, separator blocks, and repetitive step labels that
  interrupt reading. Keep concise grouping only where it improves navigation.
- Prefer a short explanation of intent, invariants, or non-obvious tradeoffs near
  the relevant code. Update stale symbol references and misleading explanations.
- Preserve safety proofs, protocol requirements, security boundaries, concurrency
  and ownership invariants, useful API documentation, and actionable TODOs with
  valid prerequisites. Preserve license, attribution, generated-file markers,
  lint directives, and comments consumed by tooling.
- Check explanations against the implementation and project authority. Do not
  change behavior to match a stale comment or delete a valid requirement merely
  because the implementation violates it; record such discrepancies separately.

## Execution Plan

1. **Baseline:** Read root/applicable nested `AGENTS.md`, start from
   `skills/SKILL.md`, and progressively load relevant architecture, review
   knowledge, and testing rules. Record HEAD, existing changes, supported build
   combinations, discovered tests, and initial check failures.
2. **Inventory:** Create a persistent file ledger. Every path receives a scope
   classification; every in-scope code file receives a semantic inspection.
   Start with `edgion-gateway`, then finish one anchor crate at a time before
   moving through the remaining crates and repository-owned automation.
3. **Assess:** Group candidates by root cause and owning module. A technical
   analyst establishes reachability and test value. Revalidate prior review
   knowledge against current code. Freeze each batch's exact edit allow-list.
4. **Implement:** Delegate implementation and independent review to separate
   subagents; the implementer cannot approve its own changes. Every technical
   subagent reads applicable original repository rules and knowledge itself.
   Complex
   batches require a plan and independent plan approval before edits. Maintain
   one serial mutation lane; cross-crate supporting edits require evidence and
   an explicit allow-list. Revalidate queued candidates after preceding changes.
5. **Verify:** Review deletion evidence, retained test assertions, and all
   applicable mandatory design rules. Run required project checks and affected
   tests. If replacement coverage is inadequate, restore or improve it before
   accepting the batch. An independent review subagent must explicitly approve
   deletion evidence, functional preservation, test coverage, and comment changes.
   After review and verification pass, commit only the batch-owned changes
   locally before beginning the next mutation batch. Never push.
6. **Reconcile:** Reconcile the ledger with the final tree, inspect newly changed
   paths, and report deletions, retained decisions, deferred candidates, and fresh
   verification results. A scanned file needs no edit when it is already useful.

## Verification and Completion

- Each implementation batch passes formatting, workspace all-target check and
  Clippy, the repository pre-commit guards, and affected tests. Run additional
  schema, observability, security, integration, or script guards as applicable.
- Never use `--all-features`: TLS and allocator features are mutually exclusive.
  Test affected supported combinations explicitly; do not enable Gateway rustls.
- Run `cargo test --all` only with explicit user authorization. Record unavailable
  integration environments and pre-existing failures honestly; an unrun required
  check cannot be reported as passing.
- Completion requires 100% disposition of the inventory and semantic inspection
  of every in-scope code file, no unresolved review findings in accepted batches,
  and successful required verification. Uncertain candidates may remain only as
  explicitly deferred items; verification blockers keep execution incomplete.
- Keep records concise: file coverage, per-batch deletion/test mappings and checks,
  and a final report with production/test line deltas and measured runtime changes
  where available. Counts are descriptive, never acceptance quotas. Do not claim
  a performance or coverage improvement without measurement.

## Authority

This document specifies future work; writing it does not start the cleanup.
The user's follow-up authorizes local commits for verified campaign batches and
forbids push. Once campaign execution is requested, do not ask for commit approval
again for these batches. Preserve unrelated work and never stage it implicitly.
All on-disk additions must be English. Applicable project design rules and
knowledge routing remain authoritative throughout the campaign.
