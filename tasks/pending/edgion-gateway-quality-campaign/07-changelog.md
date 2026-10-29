# Campaign change record

| Batch | Date | Files | Outcome | Actual improvement | Verification | Commit | Follow-ups |
|---|---|---|---|---|---|---|---|

Bootstrap: recorded clean tracked baseline and protected user artifacts; initialized provisional tracked-path inventory and unreviewed Gateway ledger. No code changes or completed batches yet.

2026-10-28 verification hold: baseline quick checks passed with existing Clippy warnings; full tests failed on four pre-existing Controller cases. Independent diagnosis reproduced three root causes, recorded D0002-D0004 and held source mutations/commits. Three candidate analyses cover 14 Gateway files pending independent acceptance; D0005 preserves uncertain legacy accounting coverage. No completed batches or commits.

Continuation: revalidated unchanged HEAD and reproduced Controller policy failures. Completed Gateway library baseline (6,341 passed, 79 failed, 19 ignored), verified candidate-related test results, and diagnosed three additional Gateway root-cause groups D0006-D0008. Preserved all assertions and production limits. No mutation/commit acceptance.

## 2026-10-29 resumed baseline verification

Completed independently reviewed 37-path R0001 repairs. Full workspace tests now pass (13,152 passed, zero failed), along with check, Clippy with existing warnings, static guards, TLS repeats and standalone MetricsLifetime. Default integration remains pending because macOS lacks six required loopback aliases (127.0.0.2–127.0.0.7); sudo needs user-local authentication. No commit or promotion of pruning candidates.

## R0001 accepted

All baseline repairs verified within 40 implementation paths. Default integration now passes: 279 passed groups, zero failed, 11 existing skips; all final guards and cleanup passed. Closed six independently verified repair tasks; retained advisory-policy and AI duplicate-field tasks. Prepare authorized local commit before promoting B0001.
