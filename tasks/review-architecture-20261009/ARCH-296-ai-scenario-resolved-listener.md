---
id: ARCH-296
status: candidate
kind: architecture-debt
priority: P3
partition: PART-442
analyst: /root/analyze_outbound
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---
# Consume the resolved AI listener port in lifecycle scenario requests

The AiBackendLifecycle and AiBackendPolicyIntegration scenarios have a validated listener contract, but their request helpers independently own a fixed copy of that listener's port. Changing the shared AI topology requires updating scenario implementation constants even after the canonical listener mapping and Gateway fixture have been updated correctly. The already-resolved invocation context is the appropriate owner and supplies the needed scalar.

## Current ownership and change witness

`test_client.rs::dynamic_port_contract` maps both scenario IDs to `EdgionBackend/SingleProvider` with `PortKind::Http`. In Gateway mode, `run_dynamic_scenario` loads `ports.json`, requires that exact HTTP field, and constructs `ScenarioCtx.test_ctx.http_port` from it. `PortConfig::require_port` rejects an unknown key or missing required field rather than using a direct-mode default. The current entry and `02_Gateway_ai-gateway.yaml` both select 31945, so the default topology presently agrees.

Both primary scenario files nevertheless declare `AI_GATEWAY_PORT: u16 = 31945` and ignore the resolved port for data-plane requests. In `ai_backend_lifecycle.rs`, `ai_request_model_with_headers` passes the constant to `target_socket_addr` and embeds it in the chat-completions URL; `model_list` repeats that pattern for `/v1/models`. In `ai_backend_policy_integration.rs`, `request_client_with_timeout` uses the constant for its host override, and `ai_request`, `capture_retry_request`, and `assert_client_abort` embed it in their request URLs. All ordinary, gated-retry, and disconnect request paths therefore share the independent constant.

A concrete change scenario is relocating the existing AI listener by updating its Gateway declaration and canonical `EdgionBackend/SingleProvider.http` mapping together. The unchanged runner successfully resolves the new port and puts it into the invocation context. These two scenarios still resolve and dial 31945. They can fail against an absent old listener or exercise an old listener rather than the declared one if that port remains occupied. This is a conditional source-level consequence of a supported topology change, not a claim that current default invocations fail or that another listener is currently present there.

The original `integration-port-selection-must-fail-closed.md` explicitly requires dynamic tests to consume their declared context port instead of repeating numeric literals. The integration guide treats `ports.json` as the canonical mapping and requires Gateway fixtures and that mapping to move together. The Kubernetes dynamic wrapper also copies the selected ports file into the remote configuration directory. Merely validating that file before ignoring its selected value does not fulfill the listener ownership contract.

The adjacent AiBackendSecretRotation scenario provides an actual bounded local precedent: its phase suite resolves the same exact key, and its existing client and request URL use `TestContext.http_port`. This proposal does not replace the static scenario inventory or introduce a new selection mechanism.

## Bounded improvement and invariants

Remove the two private port constants and borrow `ctx.test_ctx.http_port` at every affected request helper. Use that same scalar for both `target_socket_addr`/the request client's host override and the URL authority. Retain each scenario's own logical `HOST`, provider endpoint configuration, Gateway parent-reference name, timeouts, authentication, no-proxy setting, trace correlation, and assertions. Those represent legitimate scenario-specific behavior and need no consolidation.

Keep the existing exact key/protocol validation in the runner. Do not introduce a second ports-file lookup inside the scenario, an environment override, a constant fallback, or a generic HTTP-client factory. There is no production egress, TLS, body-storage, workqueue, logging, metric, or request-path change. The benefit is one canonical listener value flowing from validated selection to actual dial and URL construction, with fewer change sites when the harness topology moves; no measured performance or allocation benefit is claimed.

An alternative is a dedicated scenario listener accessor that returns the same context scalar, but it adds little value unless an owning caller genuinely needs it. Retaining constants plus a test that they equal today's registry only detects drift after maintaining duplicate owners; directly consuming the resolved value removes that obligation.

## Future verification and scope

Use a non-default port in the existing harness context and verify that the affected chat, model-list, gated-retry, and disconnect request construction uses it consistently for resolution and URL authority. Preserve the current exact-key/missing-protocol failure tests and all embedded YAML, credential, TLS, weighted-sampling, retry-capture, stream-termination, status and cleanup assertions. Such focused construction checks can remain network-free and do not require changing product visibility or weakening live integration tests.

The recommendation is limited to the two inspected primary scenario owners. It does not inventory unrelated numeric constants, change provider/backend ports, or claim that every suite has the same problem. Existing ARCH264 initialization/restoration, ARCH265 cleanup-result ownership, and ARCH266 selected-status projection have different causes and receive only separately reported supporting evidence from this partition. The primary implementations, embedded tests, real invocation/context/client seams, original decisions and fixtures were read; no build, test, network operation or mutation occurred.
