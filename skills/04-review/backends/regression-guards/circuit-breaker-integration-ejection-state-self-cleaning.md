---
name: circuit-breaker-integration-ejection-state-self-cleaning
description: Use when reviewing circuit-breaker integration fixtures that leave an endpoint ejected across repeated test-client runs on a preserved Gateway.
status: fixed
finding_id: change-fix-06
closed: 2026-08-02
knowledge_type: regression-guards
---

# Circuit-Breaker Integration Ejection State Is Self-Cleaning — FIXED

## Conclusion

The `cb-retry` integration fixture caps both the base and maximum ejection time at three seconds, and `test_status_retry_counts_each_attempt` polls for backend recovery before and after its ejection assertion. Repeated test-client runs against a preserved Gateway therefore do not inherit an unusable backend.

Reducing only `ejectionTime` without bounding `maxEjectionTime`, or treating `--no-prepare` as Gateway reuse, is **not accepted**.

## Core Rationale

**1. Preserved Gateway state is the relevant boundary**

`run_integration.sh --no-prepare` skips the build but still starts a fresh stack and cleans it up by default. Ejection state survives only when the same Gateway process is retained, such as a `--keep-alive` run followed by direct `test_client` executions or a later `--no-start` run.

**2. The maximum must be explicit**

Circuit-breaker cooldown uses `ejectionTime * num_ejections`, capped by `maxEjectionTime`, and recovery retains `num_ejections`. A shorter base duration alone still produces progressively longer waits across repeated runs; the test fixture needs an explicit equal maximum.

**3. The test owns both entry and exit cleanup**

An always-successful probe before the assertion handles state left by an interrupted prior run. A matching probe after the assertion leaves the shared fixture ready for another test while also verifying cooldown recovery.

## Fix Suggestions Not Accepted

- Change only `ejectionTime` — repeated ejections still back off toward the default five-minute maximum.
- Add a fixed sleep — polling exits promptly and verifies actual backend reachability.
- Restart the Gateway between cases — integration suites share one topology, and direct reruns intentionally preserve it.

## Re-evaluation Triggers

Re-open this decision only if:

- circuit-breaker recovery starts resetting `num_ejections`; or
- the integration harness guarantees a fresh Gateway for every direct test-client execution.

## Reference Cases

- Finding `change-fix-06`, fixed 2026-08-02.
- `edgion-tests/src/client/suites/http_route/backend/circuit_breaker.rs`.
- `edgion-tests/integration/conf/HTTPRoute/Backend/CircuitBreaker/EdgionBackendTrafficPolicy_default_cb-retry.yaml`.

## Resolution references

- `change-fix-06` — 2026-08-02.
