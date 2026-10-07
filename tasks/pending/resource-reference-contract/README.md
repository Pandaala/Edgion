# Shared resource reference contract

## Meta

| Key | Value |
| --- | --- |
| Created | 2026-10-07 |
| Status | in-progress |
| Type | refactor |
| Priority | P2 |
| Source | [Core API group alias investigation](../../todo/core-api-group-alias-contract.md) |
| Current phase | User approved IMPL implementation; implementation in progress; independent plan review passed |

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

The source investigation remains open. Approval of this specification and a
reviewed implementation plan are separate from completion of the design work.
The general review and the subsequent two focused rounds found no remaining
design blocker in the final revision. Grant consistency, reference coverage and
conformance impact each received independent final LGTM after correction.
This does not establish runtime or live conformance results. Recommended choices are approved as recorded in 00-context.md; independent
implementation-plan review remains a gate.
