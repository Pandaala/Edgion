---
id: ARCH-177
status: candidate
kind: behavior-defect
priority: P2
partition: PART-235
analyst: /root/analyze_outbound
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Propagate directory apply failures through the existing batch result

`edgion-cli/src/cli/commands/apply.rs::apply_directory` catches every `apply_file` error, prints it, and continues, but always returns `Ok(())` after enumeration. Its successful-file counter is also used as the evidence that YAML files existed, so a directory containing only failed YAML inputs additionally prints `No YAML files found`. The latter message is a consequence of the same lost operation result, not a separate finding.

The outer ownership boundary already expects errors. `apply_many` processes input paths in order, records each failed path, and returns an aggregate error after trying the remaining paths. `Cli::dispatch` returns that result, and `src/bin/edgion_ctl.rs::main` exits with status 1 only on an error. A failed file passed directly therefore fails the command, whereas the same file reached through a directory is reported as a successful input path and can leave the command with exit status 0.

A fully local witness is a directory containing one empty `bad.yaml`: `apply_file` returns `Empty YAML file`, `apply_directory` prints the failure and then returns success, and `apply_many` has no failed paths to report. Supplying a nonempty static `--token` and `--dry-run` reaches this path without authentication preflight or resource requests; client construction itself performs no request. The same result loss also applies to malformed metadata, filesystem read errors and rejected write responses propagated by `apply_file`, although this review did not execute any of those scenarios or assert a deployment incident.

Keep directory processing best effort, but track attempted YAML files separately from successful files and retain whether any apply failed. After trying all eligible files, return an aggregate error for that directory when failures occurred. The existing `apply_many` then preserves ordered best-effort processing across input paths and gives main the correct failure outcome. Report `No YAML files found` only when no eligible files were encountered. A compact failed-file count with already emitted per-file diagnostics is sufficient; retaining resource contents or introducing an operation framework is unnecessary.

Failing immediately on the first directory error is an alternative, but would discard the existing useful continuation behavior. Neither remedy should roll back successful writes, create a transaction, recurse or sort directory entries, change per-file document fail-fast behavior, or impose new resource-version preconditions. The original Admin API decision explicitly keeps CLI apply declarative and precondition-free; batch outcome propagation is independent of that accepted last-writer-wins policy.

Existing `apply_test.rs` bodies cover stream splitting and Clap's repeated input flags, but have no directory-result or `apply_many` failure tests. Future verification should distinguish no eligible files, all failed, mixed success/failure, a later input path succeeding after a failed directory, and a single file failure; verify both returned errors and the binary's exit contract without requiring live infrastructure. Preserve canonical HTTP transport, authorization, redaction and existing inline Controller storage/status ownership. No source changes or execution were performed, and no existing review decision was found accepting successful exit after failed directory inputs.
