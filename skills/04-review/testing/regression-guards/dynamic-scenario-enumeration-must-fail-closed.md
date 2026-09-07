---
name: dynamic-scenario-enumeration-must-fail-closed
description: Use when reviewing dynamic integration scenario registration, listing, or shell enumeration.
knowledge_type: regression-guards
---

# Dynamic scenario enumeration must fail closed

## Conclusion

The repository has a required dynamic hot-reload test layer. A failed listing command, empty registry,
duplicate identifier, or drift from the explicit expected manifest is an infrastructure failure.

## Core rationale

`test_client --list-dynamic-scenarios` validates the embedded capability registry, the complete expected ID
manifest, uniqueness, non-empty IDs, and listener-contract declarations before printing. The shell runner
preserves listing diagnostics, requires a zero exit status and non-empty output, and propagates failure to the
run-level result.

The default capability registry is embedded in the binary. An explicit `EDGION_CAPABILITY_REGISTRY` remains
a test override and must be readable and valid; distributed binaries must not depend on the build machine's
absolute source path.

## Fix suggestions not accepted

- `|| true` or discarded stderr around enumeration.
- Treating empty output as an intentional skip without a named policy.
- Validating only when a scenario is executed; listing is the preflight inventory boundary.
- Using only a minimum count, which cannot identify one removed and one accidentally added scenario.

## Re-evaluation triggers

If a product variant intentionally ships without dynamic scenarios, introduce an explicit variant policy and
reported skip state. Do not represent that policy as an empty successful registry.

## Reference cases

- `edgion-tests/src/client/test_client.rs`
- `edgion-tests/integration/scripts/integration/run_integration.sh`

## Resolution references

- `ITEST-012` — 2026-08-26.
