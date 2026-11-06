# arch-05: Review knowledge pages still cite the pre-bump Pingora fork revision after the move to `edgion_v0.9.1`

- **Category**: design-error (governance drift)
- **Severity**: P3 (downgraded from P2 on re-verification: the gate is green again)
- **Scope**: local
- **Location**:
  - `skills/04-review/**` — 17 pages citing `0b0d8bba` (some as `edgion_0.9.0_v1@0b0d8bba`) as the locked fork revision
  - `skills/04-review/supply-chain/mandatory-design/pingora-fork-branch-lock-policy.md:76` (historical `0b0d8bba` reference)

## Summary

The guard and project guidance were fixed: `cicd/checks/check_pingora_source_policy.py:18` now requires
`edgion_v0.9.1`, the guard passes, and `CLAUDE.md` names `edgion_v0.9.1`. What remains is evidence drift: 17
`skills/04-review` pages still state that the *locked* fork revision is `0b0d8bba` (several also name the old
branch `edgion_0.9.0_v1`), while `Cargo.lock` resolves every fork package to `e5f4287be713`. None of those pages
cites `e5f4287`, so their fork-internal conclusions (H2 end-stream observer, writer-capacity stall, shutdown
allocatability, retry loop capacity, listener port freeze, shutdown notification, handshake offload, URI
normalization, etc.) are presented as current guidance but were verified against a different revision.

## Current behavior (evidence)

```
$ python3 cicd/checks/check_pingora_source_policy.py
Pingora source policy guard passed: 16 fork packages use branch=edgion_v0.9.1 and are locked to e5f4287be713d01ffd444544251bf75c9d00062a.
```

- `grep -rl 0b0d8bb skills | wc -l` = 17; `grep -rl e5f4287 skills | wc -l` = 0.
- Old branch name still in evidence lines: `h2-grpc/regression-guards/h2-proxy-cache-reuse-harness-consumers.md:48`,
  `h2-shutdown-connection-not-allocatable.md:36`, `h2-writer-capacity-stall-after-response.md:44`,
  `h2-end-stream-wire-observer.md:59`.
- Pages asserting "locked revision 0b0d8bba" as present fact include
  `concurrency/regression-guards/proxy-shutdown-notification.md:11`,
  `tls/regression-guards/downstream-handshake-offload-budget-and-cancellation.md:18`,
  `listeners/compatibility-constraints/listener-port-set-frozen-at-startup.md:37`,
  `plugin-runtime/compatibility-constraints/filter-iteration-control-sync-phase-tradeoff.md:11`,
  `routing/regression-guards/retry-budget-owns-pingora-loop-capacity.md:29`.

## Impact

- Reviewers following the knowledge-routing rules treat these pages as evidence; their cited revision is no
  longer the one built, so a regression introduced by the fork bump would not be caught by re-reading them.
- No runtime impact by itself.

## Suggested direction

1. Re-verify each of the 17 fork-internal pages against `edgion_v0.9.1@e5f4287` and update the cited revision
   (or mark the old revision explicitly as historical evidence where it was a one-time observation).
2. Optional: have the guard print the locked commit (it already does) and add a doc check that flags
   `skills/04-review` pages claiming a "locked" revision different from `Cargo.lock`.

## Notes

- The original guard failure (seven `development branch must be edgion_0.9.0_v1` errors) is fixed.

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P2
- Ran `python3 cicd/checks/check_pingora_source_policy.py`: exit 1, seven `development branch must be edgion_0.9.0_v1` errors.
- `Cargo.toml:124-130`: all seven fork deps use `branch = "edgion_v0.9.1"`; `Cargo.lock:8` resolves `e5f4287be713...`.
- `cicd/checks/check_pingora_source_policy.py:18`: `FORK_BRANCH = "edgion_0.9.0_v1"`; `CLAUDE.md:48,246` still name the old branch.
- `grep -rl 0b0d8bb skills | wc -l` = 17 pages citing the old fork commit as evidence.

## Re-verification (feature-10-28)

- **Verdict**: PARTIALLY-FIXED
- **Severity**: P3 (was P2)
- `cicd/checks/check_pingora_source_policy.py:18`: `FORK_BRANCH = "edgion_v0.9.1"`; guard exits 0 (16 fork packages locked to `e5f4287be713`).
- `CLAUDE.md` names `edgion_v0.9.1`; no `edgion_0.9.0` remains in `CLAUDE.md`; `pingora-fork-branch-lock-policy.md:18,48` updated to the new branch.
- Remaining: 17 `skills/04-review` pages still cite `0b0d8bba` as the locked revision (4 with branch `edgion_0.9.0_v1`); no page cites `e5f4287`.
