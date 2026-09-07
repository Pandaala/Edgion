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

## Re-evaluation Triggers

Re-open this decision only if `run_integration.sh` replaces its duplicated static sweeps with one
canonical suite registry or changes the outer `run_all_tests` branch structure.

## Reference Cases

- `integration-suite-registration-guard` (2026-08-30)
- `edgion-tests/src/client/test_client.rs`
- `skills/05-testing/01-integration-testing.md`

## Resolution references

- `integration-suite-registration-guard` — 2026-08-30.
