---
name: dynamic-scenario-state-restoration
description: Review ordinary-error restoration in shared dynamic scenarios and distinguish lifecycle tests from live recovery evidence.
knowledge_type: regression-guards
---

# Dynamic scenarios own restoration

BasicAuthDynamic, BasicAuthDynamicGrouped, BodyDynamic, BtlspRefFlap,
CanaryHotSwap and ConditionRefsDynamic establish their initial fixtures before
polling business behavior. Each captures setup/transition errors before entering
its own restoration block. This includes a directory apply that changed some
resources before returning an error. The shared apply helper remains fail-fast;
restoration instead attempts each independently owned resource separately.

Body restores both the shared gateway policy and its original plugin. Its
existing `restored` phase still tests the updated plugin after lifting the cap;
completion additionally polls the original `initial` behavior. Canary restores
both selector and plugin; stable routing alone cannot establish that the updated
release-track dye disappeared. Its initial suite checks original stable dye and
absence of release-track dye as well as the original backend assertion.

ConditionRefs restores all five current Secret/IP-list resources independently.
Secret and live-IP recovery are checked independently of the RealIP group.
Trusted primary RealIP can mask a stale secondary, so `secondary-restored`
additionally reads that non-secret resource's authored spec through the existing
Admin endpoint. It does not infer secondary recovery from trusted traffic.
Controller-owned currentStatus is excluded and absent enable is normalized to
its true schema default; all other spec content must equal the initial fixture.

The owner returns both the original failure and restoration errors. A failed
restoration after otherwise successful transitions is still a failed scenario.
An unavailable reset prerequisite suppresses only its dependent business check;
it does not prevent other reset attempts or independent checks. These guarantees
cover ordinary returned errors while the future runs, not cancellation, panic,
process death, unavailable infrastructure or concurrent mutation by another owner.
Outer local/Kubernetes topology cleanup retains its separate responsibility.

## Topology and regression evidence

Body and Canary have different initial fixture filenames in the local and
Kubernetes trees. Their explicit pairs require exactly one existing candidate.
Missing or ambiguous fixtures fail rather than silently skipping restoration.
Other owned reset paths match both trees. No mode inference from the directory
name is valid: the Kubernetes runner stages its own tree at the normal integration
root inside the pod.

`edgion-tests/tests/unit/client/dynamic/restoration_test.rs` drives the actual six
scenario futures with per-context, test-only operation interception. It covers
setup/transition errors, independent reset attempts, cleanup-only and combined
errors, recovery-barrier errors, original phase ordering, and both fixture trees.
Scenario-local tests cover missing/ambiguous filename pairs and reject stale
secondary specs. Canary tests reject stable routing with stale release-track dye.
These tests are network-free orchestration/predicate evidence, not proof of live
Controller publication, gateway behavior or Kubernetes ACK execution.

Run `cargo test -p edgion-tests --lib dynamic::` and
`cargo test -p edgion-tests --lib canary::dynamic::restoration_tests`.
Live acceptance requires consecutive direct invocations without preload between
them, mutation failure injection, and original business checks before another
scenario establishes fixtures. Consult the implementation verification record
for actual executed checks and remaining gates; no historical run is implied here.

Kubernetes ACKs may repeat existing phase names. A scenario with an inventory
file requires a row for every emitted phase; absence of the file is separately
allowed. Canary retains initial/final inventory rows; ConditionRefs currently
has no inventory file. Re-evaluate this boundary when adding an inventory or
changing recovery phases, resources, fixture names, or masking relationships.

## Resolution references

- ARCH-264: scenario-owned establishment and restoration for these six IDs.
- Authority: [integration testing](../../../05-testing/01-integration-testing.md)
  and [dynamic Secret fixture discipline](../mandatory-design/dynamic-secret-fixtures-must-be-tracked.md).
