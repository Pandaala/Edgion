---
name: integration-suite-registrations-must-be-checked-per-sweep
description: Use when reviewing static integration suite registration guards for the targeted resource and default runner sweeps.
status: fixed
finding_id: integration-suite-registration-guard
closed: 2026-08-30
knowledge_type: regression-guards
---

# Integration suite registrations must be checked per sweep — FIXED

## Conclusion

A suite required in both `run_integration.sh` paths is counted independently in the targeted
resource dispatch and the default full sweep. Each applicable region must contain exactly one
registration.

Fix suggestions that only assert a whole-file occurrence count are **not accepted**.

## Core Rationale

**1. A whole-file total does not prove placement**

Two occurrences may both be in one branch. That can silently remove a suite from `-r <Resource>`
selection even though the aggregate count remains unchanged.

**2. The runner regions are the behavioral boundary**

The targeted and default branches have different callers. The guard parses the outer branches of
`run_all_tests` once and shares those regions with both exact-registration and targeted-to-default
coverage checks.

**3. Synthetic inputs lock the failure modes**

Unit regressions cover a correctly split registration, a duplicate targeted registration, a
missing default registration, and a missing targeted registration hidden by duplicate defaults.

## Fix Suggestions Not Accepted

- Count the suite name across the whole script — placement remains unproven.
- Change suite ownership or runner ordering — neither is required to fix the guard.
- Require exact one-to-one registration for every aggregate suite — targeted bundle names may be
  intentionally covered by multiple finer-grained default entries.

## Fixture preload coverage

The EdgionPlugins fixture preload guard additionally compares complete directory
names or recursive parent directories. Substring matching is insufficient:
`EdgionPlugins/DslBody` does not load `EdgionPlugins/Dsl`, while
`EdgionPlugins/PluginCondition` does cover its `AllConditions` child. The
`suite_preload_requires_exact_directory_or_recursive_parent` regression captures
these boundaries.

The inverse must also hold: every explicit `EdgionPlugins/*` preload entry must
name a real fixture directory. A logical runner item may reuse a parent fixture
directory instead of owning a same-named directory. In particular,
`EdgionPlugins_DslGrpcError` is an execution item backed by
`EdgionPlugins/Dsl`; adding a synthetic `EdgionPlugins/DslGrpcError` preload
causes `load_conf.sh` to reject the nonexistent directory. The resource-wide
guard therefore checks both missing coverage and nonexistent explicit preload
entries.

These guards check static directory coverage, not successful YAML submission or
runtime behavior; see the
[configuration failure gate](mandatory-integration-gates-fail-closed.md).

Current verification (2026-09-15): the `test_client` unit-test binary passed all
22 tests, including resource preload coverage, explicit preload existence, and
exact/recursive directory matching. A live `-r EdgionPlugins` sweep loaded the
inventory, and every executed plugin suite passed; the metrics-label,
unknown-field, and requeue final gates also passed. Its report recorded 64 PASS
entries, one SKIP, and one separate cache-residue
failure, which is tracked by
`wasm-shared-data-eviction-grace-exceeds-residue-timeout`. A focused
`-r EdgionPlugins -i DslGrpcError` run also passed with the ordinary residue gate.

## Re-evaluation Triggers

Re-evaluate if `run_integration.sh` replaces its duplicated static sweeps with one
canonical suite registry, changes the outer `run_all_tests` branch structure, or
changes fixture selection and recursive directory loading semantics.

## Reference Cases

- `integration-suite-registration-guard` (2026-08-30)
- `edgion-tests/src/client/test_client.rs`
- `skills/05-testing/01-integration-testing.md`

## Resolution references

- `integration-suite-registration-guard` — 2026-08-30.
- `edgionplugins-resource-sweep-config-list-drift` — bidirectional fixture preload coverage.
