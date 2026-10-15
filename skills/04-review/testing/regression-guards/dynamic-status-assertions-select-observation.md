---
name: dynamic-status-assertions-select-observation
description: Use when reviewing dynamic scenario status barriers and Controller or attachment selection.
knowledge_type: regression-guards
---

# Dynamic status assertions select their observation

## Conclusion and scope

Dynamic status barriers must read the scenario's observation before matching a
condition. Complete Admin resource status is diagnostic history, including other
Controllers and attachments. A recursive condition search cannot establish that
the intended observer reached a phase. Independent traffic assertions remain
necessary; an unsound individual barrier does not prove a whole scenario can
falsely pass.

`edgion-tests/src/client/dynamic/status.rs` owns the bounded projection used by
AiBackendLifecycle, AiBackendWeighted and AiBackendPolicyIntegration. Simple
EdgionBackend conditions use native status locally and the selected
`edgion.io/gateway-controller` observation in Kubernetes. Kubernetes requires
exactly one selected observation. Route parents and both backend Policy ancestor
families require exactly one entry with that Controller and the expected
canonical Gateway reference in either topology. Missing or ambiguous observations
and missing conditions fail the selection; absence cannot prove rejection.

## Rationale and evidence

The [status architecture](../../../01-architecture/01-controller/10-status-management.md)
retains complete status and separately transports currentStatus. The
[canonical parent ownership contract](../../status/regression-guards/canonical-parent-status-ownership.md)
defines reference identity, including group/kind/namespace defaults and exact
name, section and port. Reuse `ParentReference::canonical_identity`; matching
only a Gateway name or choosing any parent/ancestor loses this boundary.

Lifecycle and Weighted expect `edgion-test/ai-gateway`. PolicyIntegration uses
its existing `gateway_name()` selection, including the Kubernetes
`policy-integration-gateway` override. Lifecycle's Policy namespace differs from
the Gateway namespace; never default the expected ancestor to the Policy's
namespace. Policy ancestors identify Gateways, not target backends. The producer
resolves those ancestors through backend-to-Route-to-Gateway dependency edges.
PolicyIntegration creates a Route referencing its absent backend before asserting
the missing-target Policy outcome; no synthetic ancestor fallback is necessary.

ARCH-266 identified recursive predicates in these three AI scenarios. Their
replacement selects the observation and then preserves the existing
type/status/optional-reason predicate. Requests, authentication, response errors,
deadlines, cleanup, traffic and publication assertions keep their existing
semantics. The original BodyDynamic/PmdRefFlap flat-condition subclaim was already
resolved before this correction: those readers use
`suites::status::helpers::controller_local_conditions` and were left unchanged.
The broader status-suite `has_condition` still searches any parent/ancestor and
is not a substitute for this attachment selector.

## Verification and limits

Pure tests in the dynamic status module cover native/Kubernetes simple projection,
contradictory foreign observations, missing/duplicate owners, canonical defaults,
all attachment identity dimensions, missing/duplicate attachments, reason matching
and cross-namespace Policy ancestry. Tests in each migrated scenario exercise the
same wrapper used by its wait; PolicyIntegration includes a non-default Gateway.
Run with `cargo test -p edgion-tests --lib dynamic::status` and the three
`dynamic::scenarios::ai_backend_` module filters. Existing shared-controller
projection tests run with the `controller_status_tests` filter.

Pure selector tests do not prove config publication or live local/Kubernetes
success. Consult the implementation verification record for executed results;
run the existing scenarios through the supported shared topology when available.
No status producer, currentStatus, foreign-history, fixture, body/TLS, workqueue
or observability behavior is changed by this test-harness correction.

Re-evaluate when scenario Gateway/controller identities, status shapes or attachment
cardinality change. Multiple intended attachments need an explicit scenario
predicate rather than arbitrary first-entry selection or recursive matching.

## Resolution references

- ARCH-266: shared test-owned status projection and migration of the three AI
  barriers; Body/Pmd pre-existing correction retained.
