---
name: integration-port-selection-must-fail-closed
description: Use when reviewing ports.json, TestSuite port_key declarations, or integration topology overrides.
knowledge_type: regression-guards
---

# Integration port selection must fail closed

In Gateway mode, a missing or malformed `ports.json`, an unknown suite key, or an absent primary
protocol field is an infrastructure failure. Never substitute the test client's direct-mode
defaults: those ports may belong to the backend and can make a Gateway test bypass its listener.

Each suite listener contract names an exact key and protocol kind. Resolution failure skips that
suite, records a run-level error, and makes the process fail. Keep the inventory guards that scan
declared keys and registered suites against `ports.json`. Suites that inspect only control-plane
status may be explicitly classified as requiring no data-plane listener; this is not a generic
fallback.

Mixed aggregate selections attach the contract to every child suite; one aggregate-level context
cannot represent listeners for multiple protocols or ports. Dynamic tests consume their declared
context port instead of repeating numeric literals. Kubernetes mounts a ConfigMap generated from
the canonical integration `ports.json`; an embedded copy will drift.

## AI scenario request targets

ARCH-296 removes the private 31945 port copies from `AiBackendLifecycle` and
`AiBackendPolicyIntegration`. Lifecycle chat and model-list probes use
`ScenarioCtx.test_ctx.http_port` for both target resolution and URL authority.
PolicyIntegration's shared client resolver and its ordinary, gated-retry and
disconnect request URLs consume the same context scalar. Logical hosts,
provider ports, deadlines, authentication and assertions remain scenario-owned.

The runner still requires the exact `EdgionBackend/SingleProvider` HTTP mapping
before constructing these Gateway-mode contexts. No second registry lookup or
fallback is added. Source tracing with a hypothetical non-default context port
shows it reaches every affected resolution/URL site; this is inspection, not an
executed relocated-listener test. Existing fixture and listener-contract tests
remain separate evidence, and no live topology move is claimed. Re-evaluate if
either scenario gains a second listener or request-construction path.

## Resolution references

- `ITEST-010` — 2026-08-26.
