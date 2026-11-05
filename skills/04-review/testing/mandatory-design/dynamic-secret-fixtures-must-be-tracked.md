---
knowledge_type: mandatory-design
---

# Dynamic Secret fixtures must be tracked

`edgion-tests/.gitignore` excludes `integration/conf/**/Secret*.yaml` because
that basename is reserved for dynamically generated certificate material.
Static test-only Secret manifests must use the numbered fixture convention,
for example `00_Secret_example.yaml`, so they remain visible to Git.

A dynamic scenario that applies a static Secret should also reference the
numbered filename explicitly. When the fixture is essential to the scenario,
an `include_str!` unit test should compile it into the test binary and assert
the transition-defining values. This makes a missing clean-checkout fixture a
compile failure instead of a late integration-test failure.

Do not add a broad negated ignore rule for one scenario. It would make future
generated Secrets eligible for accidental commits.

Dynamic scenarios share one Controller and Gateway and may run twice under
`--with-restart`. A mutation scenario must establish its initial state and
restore it before returning, including after a failed transition assertion, so
later suites and later sweeps do not inherit its final state.

See [scenario-owned restoration](../regression-guards/dynamic-scenario-state-restoration.md)
for partial setup, independent reset attempts, masked state, topology-specific
fixture paths and the limits of network-free versus live recovery evidence.

## Resolution references

- `test-02` — 2026-08-09.
