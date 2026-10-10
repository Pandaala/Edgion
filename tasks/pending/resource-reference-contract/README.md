# Shared resource reference contract

## Meta

| Key | Value |
| --- | --- |
| Created | 2026-10-07 |
| Status | in-progress |
| Type | refactor |
| Priority | P2 |
| Source | [Core API group alias investigation](../../todo/core-api-group-alias-contract.md) |
| Current phase | Implementation batches through R12b committed; R13 documentation reconciliation and final acceptance remain in progress |

## Current status (2026-10-10)

The design is accepted and implementation batches through R12b are committed
locally. R12a fixes valid Kubernetes object-name rejection; R12b retires obsolete
detached TLS constructors. [07-progress.md](07-progress.md) records exact commits,
scoped reviews/tests and their limits. R13 reconciles current documentation with
historical checkpoints. Affected final whole-contract cross-checks and F2 fresh
admission, dynamic traffic and full Standard conformance remain open. The original
alias investigation is retained until FULL acceptance; no push has occurred.

## AI guide summary

Design one runtime-independent reference module shared by Resources, Controller,
and Gateway. Preserve Gateway API field contracts and existing omitted-field
workflows while explicitly defining the boundaries of Edgion input conveniences.
Identity validation, authorization, dependency registration, material lookup, and
Gateway consumption must agree. The user approved implementation after design review. The independent plan
review gates production changes; final review and required checks gate closure.

## Requirements and scope

- Inventory all implemented resource reference families, including route filters.
- Distinguish object references, type declarations, attachment identities, and selectors.
- Centralize reusable semantics without replacing wire schemas or runtime stores with a loose universal reference.
- Explain omitted, empty, explicit core, custom-group, and required-field behavior.
- Preserve standard defaults and evaluate field-specific convenience support.
- Obtain multiple independent Astra design reviews and revise the specification.
- Implement the approved finite contract without new Gateway API features.
- Delegate serial implementation batches through IMPL and commit each verified
  batch locally, as newly authorized by the user. Never push.

## Documents

| Document | Purpose |
| --- | --- |
| [00-context.md](00-context.md) | Implementation authority, approved choices and baseline |
| [04-implementation.md](04-implementation.md) | Reviewable serial implementation plan |
| [06-subtasks.md](06-subtasks.md) | Subtasks and frozen candidate inventory |
| [07-progress.md](07-progress.md) | Current completion, remaining batches, commit and closure gates |
| [01-spec.md](01-spec.md) | Current-state evidence, proposed API and contracts, migration and verification plan, design review outcomes |
| [02-field-manifest.md](02-field-manifest.md) | Finite standard/custom field profiles, proposed conveniences, fixed refs, conditions and selectors |
| [03-specialized-checks.md](03-specialized-checks.md) | Two additional rounds in three directions: Grant consistency, reference coverage and conformance impact |
| [05-issues.md](05-issues.md) | Current design and verification blockers with resume conditions |

The general design reviews and two focused rounds found no remaining design
blocker after correction. Their LGTM is design evidence, not runtime or live
conformance acceptance. Approved choices remain in [00-context.md](00-context.md);
each implementation batch required independent plan/result review and checks.
The source investigation remains open until the final acceptance gates pass.
