---
name: body-suite-sweep-capability-contract
description: Use when reviewing body-suite integration registration and stock-gateway skips; covers inventory completeness and capability-driven execution.
status: fixed
finding_id: BODY-005
closed: 2026-07-27
---

# Body Suite Sweep and Capability Contract — FIXED

## Conclusion

Body-buffering suites must remain registered in both normal integration sweeps
and the resource-wide configuration inventory. A stock gateway must discover
the same suites and report capability skips instead of silently omitting them.

Fix suggestions that maintain separate incomplete suite inventories or remove
unsupported suites before capability evaluation are **not accepted**.

## Core Rationale

**1. Normal sweeps are the regression boundary**

An explicitly selectable suite does not protect normal CI if the default and
`-r EdgionPlugins` paths never invoke it. `DslBody` and `BodyPlugins` therefore
appear in the resource sweep, default sweep, and resource-wide configuration
list. The runner-level test counts each registration to detect omission.

**2. Unsupported modes need visible success**

`capabilities.tsv` declares both suites as requiring `body-buffering`. The
stock binary reports that capability as disabled, so non-explicit sweeps record
`SKIP` and continue successfully. Explicitly requesting an unsupported suite
still fails, preventing a user's targeted request from becoming a false green.

**3. Flavor names are an end-to-end contract**

The integration runner, preparation scripts, and shared starter use `body` and
`stock`. A stale flavor label prevents the stock gateway from starting before
capability evaluation, while an undefined logging helper turns a valid skip
into a shell failure. Lightweight assertions pin both seams.

## Fix Suggestions Not Accepted

- Run body suites only through explicit item selection — normal CI would not
  exercise the body runtime.
- Delete body suites from stock inventories — unsupported coverage would become
  invisible instead of an explicit capability skip.
- Treat an explicitly selected unsupported suite as successful — this would
  hide a mismatch between the user's request and the tested binary.

## Re-evaluation Triggers

Re-open this decision only if:

- Integration suite and configuration inventories become generated from one
  canonical registry.
- The stock binary gains body-buffering support.
- Capability selection moves out of `test_client`.

## Reference Cases

- `BODY-005`
- `edgion-tests/integration/capabilities.tsv`
- `edgion-tests/integration/scripts/integration/run_integration.sh`
- `edgion-tests/integration/scripts/utils/start_all_with_conf.sh`
- `edgion-tests/src/client/test_client.rs`
