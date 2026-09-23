---
name: mandatory-integration-gates-fail-closed
description: Use when reviewing integration validation prerequisites, skips, or infrastructure failures.
knowledge_type: regression-guards
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

The cache-residue validator also treats the store-stats response envelope as a
mandatory prerequisite. Both Controller and Gateway responses must contain
`success: true` and an object-valued `data` field before any counts are walked.
An HTTP 200 response with `success: false`, a missing/non-boolean `success`, or
missing/null/non-object `data` is an infrastructure error, never an empty cache
snapshot. This prevents an unavailable or contract-broken diagnostics handler
from making the final residue gate pass without checking either process.

## Configuration submission and log pipelines

`start_all_with_conf.sh::load_configs` submits each selected suite through
`load_conf.sh` and captures its output with `tee`. Both processes must succeed:
without `pipefail`, a rejected YAML batch is hidden by a successful log write,
leaving `failed=false` and allowing startup to proceed with incomplete fixtures.
The loader now scopes `pipefail` to that pipeline's subshell, preserving caller
options and the remaining script's pipeline semantics. Failures identify the
suite and log path; remaining suites are attempted, then the aggregate failure
aborts startup before Gateway tests run. Log sink failures are also fatal.

The original `edgionplugins-resource-sweep-config-list-drift` task described
missing fixture inventories and historical HTTP 404 failures. The corrected
resource sweep has directory coverage for every executed item and rejects
explicit preload entries that do not name a real fixture directory.
`PluginCondition/AllConditions` is covered recursively by its parent, and the
logical `DslGrpcError` execution item is covered by `EdgionPlugins/Dsl` rather
than a nonexistent same-named directory. Keep execution registration evidence in the
[suite-registration guard](integration-suite-registrations-must-be-checked-per-sweep.md).

Live verification on 2026-09-15 loaded the complete EdgionPlugins inventory;
every executed plugin suite passed, including the historically failing suites
and `DslGrpcError`; the metrics-label, unknown-field, and requeue final gates
also passed. The report recorded 64 PASS entries, one SKIP, and one final
cache-residue failure caused by a distinct Wasm delayed-eviction deadline
mismatch. The validator deadline was raised above the then-current 60-second
module eviction grace (subsequently removed by plugin ownership); that historical result does not
invalidate configuration-submission or suite behavior evidence. A focused
`DslGrpcError` run subsequently passed its ordinary cache-residue gate as well.

Verification: `bash cicd/checks/tests/integration_config_load_test.sh` passes
success, loader exit 7 with a successful log sink, log sink exit 9 with a
successful loader, continued collection after one failed suite, and caller
shell-option isolation. These isolated checks use the actual loader function
with temporary fixtures; they do not prove live Controller/Gateway integration.
Re-evaluate when configuration submission, output capture, or startup error
propagation changes.

## Resolution references

- `ITEST-005` — 2026-08-26.
- `edgionplugins-resource-sweep-config-list-drift` — configuration failure propagation.
- `cache-residue-store-stats-envelope-fail-open` — strict Controller/Gateway
  envelope validation and malformed-envelope regression coverage.
