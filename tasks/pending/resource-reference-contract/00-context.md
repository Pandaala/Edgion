# Implementation context

## Current authority and status (2026-10-10)

Implementation through R12a `f6b71a5217ce868e198b315ced6b8cff4c6f1b45` and R12b
`6b5adb0ae6ef40b890c4ff95247c762d13697a62` is committed locally.
R13 is documentation reconciliation; affected final whole-contract cross-checks
and F2 fresh admission/dynamic/full Standard conformance remain open. The original
alias issue stays intact until FULL acceptance. The current ledger is
[07-progress.md](07-progress.md); historical environment failures below do not
supersede its current checks or the authorized serial IMPL workflow.


The user authorized IMPL implementation on 2026-10-07 after three general Astra
reviews and two focused rounds in three directions. This supersedes the earlier
design-only authority. The latest user instruction authorizes committing the
current checkpoint and each subsequent verified batch, without pushing.
It also requires implementation subagents using IMPL, with the root coordinating
requirements, reviews, verification, staging and commits. Keep one serial mutation
lane. Reviewers remain independent and read-only. Deployment and unrelated cleanup
are not authorized. This instruction supersedes earlier no-commit statements;
those statements remain historical evidence of the earlier authority.

Historical implementation-start baseline: 73c1d7e81. There were no tracked changes;
the original source task and five design documents were then untracked artifacts.
That initial state is preserved as history, not a description of the current tree.
Retain the original issue until the complete authorized task passes FULL acceptance.

## Approved recommendations

- Implement the exact finite custom convenience manifest.
- Preserve standard and ACME/EdgionTls strict fields and their respective defaults.
- Reject unregistered standard backend pseudo-kinds; introduce no new mode carrier.
- EGC outbound CA/client fields use exact cluster administrative selection authority,
  requiring explicit target namespace. EGC writers select these materials; never
  invent a source namespace or widen namespaced/grant-gated fields.
- Include inline plugin Service owner-namespace correction and ForwardAuth local TLS
  wiring with dependency/authorization/status/runtime coverage.
- Preserve separate Controller/Gateway malformed EndpointSlice fallback policies.

## Baseline and execution

The following environment notes describe the implementation-start checkpoint.

The initial document guard failed on the stale OpenAI request.rs source link
recorded in 05-issues.md. That owning link was repaired; subsequent R12a/R12b root
document gates passed. The old failure is provenance, not a current blocker.
Saved conformance reports are historical, not verification of this implementation.

The unified shell runner initially failed with OS error 24. Node fs/promises and
child_process.execFile worked as fallback. The normal runner recovered before
production edits; this does not justify skipping checks.

See [04-implementation.md](04-implementation.md), [06-subtasks.md](06-subtasks.md),
[01-spec.md](01-spec.md) and [02-field-manifest.md](02-field-manifest.md).
The current batch and closure ledger is [07-progress.md](07-progress.md).
