# Shared resource reference contract

## Meta

| Key | Value |
| --- | --- |
| Created | 2026-10-07 |
| Status | in-progress |
| Type | refactor |
| Priority | P2 |
| Source | [Core API group alias investigation](../../todo/core-api-group-alias-contract.md) |
| Current phase | Implementation and R13 committed; weighted Kubernetes blocker corrected and verified; remaining acceptance stays open |

## Current status (2026-10-27)

The implementation and R13 documentation reconciliation are committed. Fresh
Kubernetes checks on source `5e359f300` passed 34 API-server admission cases,
22 of 23 targeted runtime checks, and all four selected supported dynamic scenarios.
Long-name HTTP/TLS, certificate and backend Grant revocation/recovery, and CA
reference failure/recovery now have live evidence. F2-K8S-WEIGHT-01 is corrected
in the working tree: ordinary rules reach native weighted selection while real AI
admission remains strict. Two independent reviews, including the requested Astra
review, returned LGTM. Current-binary Kubernetes PartiallyInvalid passed 4/4,
including mixed invalid kinds, all-invalid and zero-weight controls. The original
failing evidence is retained separately from the corrected verification.
See [07-progress.md](07-progress.md) and [05-issues.md](05-issues.md) for exact
scope, current evidence, unsupported ACME status seeding, and remaining acceptance.
The original alias investigation remains open until FULL acceptance.

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
