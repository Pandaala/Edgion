---
name: mandatory-integration-gates-fail-closed
description: Use when reviewing integration validation prerequisites, skips, or infrastructure failures.
---

# Mandatory integration gates fail closed

A required integration validation is successful only when it actually runs and proves its
invariant. An unavailable metrics or diagnostics endpoint, missing authentication material,
unreadable required log, missing validator, or validator infrastructure error is a failed gate,
not a skip or pass.

Skips require an explicit operator-facing opt-out and must be recorded with its reason. The main
runner currently permits `--skip-residue-check` (or `SKIP_RESIDUE_CHECK=true`) for cache-residue
validation and `RESIDUE_MODE=warn` for a deliberately non-gating residue policy. Missing
prerequisites never activate those policies implicitly.
