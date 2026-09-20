# Edgion Gateway Quality Campaign

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-19 |
| Status | in-progress |
| Type | refactor |
| Priority | P2 |
| Baseline | `a0b4d49b298149422c7e46150a0ca027ad5321f7` |
| Active anchor crate | `edgion-gateway` |

## AI guide summary

Resume the file-by-file quality campaign for the remaining `edgion-gateway` paths in the exact order supplied by `task/optimization-by-files.md`. Each batch receives an independent value analysis, separate implementation when warranted, independent review, full required verification, and an automatic local commit. Existing integration-test files remain byte-for-byte frozen and the campaign never pushes.

## Scope

### In scope

- The residual `edgion-gateway` files listed in `task/optimization-by-files.md`, in listed order.
- Directly related crate-local unit tests approved by the batch analyst.
- Exact cross-crate support paths only when the analyst proves they are required for the anchor functionality.
- Campaign bookkeeping under this directory.

### Out of scope

- Previously processed `edgion-gateway` files not present in the residual manifest.
- Existing `edgion-tests/**` files, which are frozen and may only be executed.
- Independent cleanup of any other crate.
- Pushes and unrelated user-owned work.

## Campaign state

- Campaign status: `in-progress`
- Ordered crate phases: `edgion-gateway` (active); later crates are not authorized by this residual run.
- Completed crates: none recorded in this campaign.
- Active serial batch: none; B0064 passed implementation, independent review, and all commit gates and is ready to commit.
- Analysis queue: none; the next residual file remains `pending` until B0064 commits.
- Current coverage counts: 582 ledger rows; 504 imported `excluded-with-reason`, 12 `pending`, 1 `deferred`, 39 `changed`, 25 `examined-no-change`, and 1 `removed`.
- Frozen integration baseline: 3,442 paths captured in `integration-freeze.tsv`; 15 fingerprints were refreshed to match the externally committed `94c56632a` test changes before B0061 review and verification.
- Resolved external gate: `RUSTSEC-2026-0285` was remediated by locking
  `rustls 0.23.45`; the remediation run passed full `cargo deny check` on
  2026-09-19. B0001 remains `in-progress` until the campaign workflow resumes.

## Protected pre-existing work

- `task/optimization-by-files.md` (user-supplied campaign order; read-only and not staged)
- `tasks/pingora-0.9.0/` (all current untracked contents)
- `tasks/todo/complete-k8s-dynamic-scenario-parity/` (all current untracked contents)
- `waf-architecture-and-multi-pattern-matching.md`

## Governing constraints

- Preserve external behavior, configuration, protocol semantics, and architecture unless a batch value case explicitly proves a local correction is required.
- Apply all red design rules, including binary-owned logging, canonical access-log ownership, redaction, observability restraint, and cross-cutting unification.
- Keep one serial mutation lane and use distinct analysis, implementation, and review agents.
- Do not modify existing `edgion-tests/**` files.
- Commit each completed batch locally after all gates pass; never push.

## Document index

| Document | Status | Notes |
|----------|--------|-------|
| `00-context.md` | done | Campaign workflow and repository knowledge entry points |
| `03-subtasks.md` | in-progress | Full inventory reconciled with the residual manifest |
| `07-changelog.md` | in-progress | Unified batch audit trail |
| `deferred/INDEX.md` | in-progress | Coordinator-owned deferred issue index |
| `integration-freeze.tsv` | done | Frozen existing integration-test manifest |

## Coverage plan

- Preserve the exact 78-line residual manifest order; do not path-sort pending work.
- Treat tightly coupled files as shared analysis units while retaining their individual ledger order.
- Inspect cross-module ownership at backend/LB/proxy, config/runtime stores, plugin/session/Pingora callbacks, TLS routing/BoringSSL callbacks, LinkSys/common egress, and metrics/inventory boundaries.
- The 504 current non-listed files are imported as prior coverage with unavailable historical per-file outcomes; no fresh technical verdict is asserted for them.
- `edgion-gateway/benches/lru_expiration.rs` is terminal `removed`, with deletion provenance from commit `5a25d32e880c89cfa9af4f28008d1bf34bb7574c`.
