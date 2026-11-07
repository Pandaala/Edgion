---
name: retained-listener-manager-clearing-wire-coverage
description: Use when reviewing retained listener route-manager clearing or its HTTP, gRPC, and TCP dynamic integration coverage.
knowledge_type: regression-guards
---

# Retained listener managers stop serving removed routes

## Conclusion and scope

An existing listener proxy retains the per-listener route-manager `Arc` selected
at construction. Removing the manager from the global registry must publish an
empty route table to that old handle before it is dropped. The
`RetainedListenerHandles` dynamic scenario checks the routing consequence
through the original HTTP, gRPC, and TCP listeners after their Gateway and
Routes are removed. It also checks that recreating the same `ListenerKey`
populates new registry entries while the original listener still cannot reach
the backend.

## Rationale and evidence

`runtime/server/listener_builder.rs` passes manager handles to the serving
proxies. HTTP and gRPC managers clear removed handles in their full stale and
partial orphan sweeps; TCP uses the shared `routes/l4_rebuild.rs` helpers.
Their unit tests retain the old `Arc` and cover both sweep branches. The
cross-process scenario in
`edgion-tests/src/client/dynamic/scenarios/retained_listener_handles.rs`
starts with successful backend traffic, deletes the Gateway and Routes, polls
configuration publication and routing outcomes, and then recreates the same
keys. Its store-stat comparisons show that the new registry entries contain
routes while the original listeners remain unable to reach their backends;
they do not directly inspect the old manager handles.

The exact `-r Gateway -i RetainedListenerHandles` runner selection originally
sent this dynamic ID to the ordinary Gateway suite, which returned `Unknown
suite`. `run_integration.sh` now invokes `test_client -g --dynamic-scenario
RetainedListenerHandles` for that item. If a scenario failure occurs during
removal or recreation, cleanup reapplies the initial fixtures before deleting
them so a prior delete does not turn the cleanup into a misleading 404.

## Alternatives and limits

Controller requeues and Gateway configuration streams can interleave, so the
wire scenario cannot force the full stale sweep or partial orphan sweep.
Each kind arrives on its own stream, so an HTTPRoute delete can be applied
before its Gateway delete. The partial update then keeps the still-declared
listener entries, and only the later full listener rebuild retires them. With
several hundred cached routes that rebuild takes hundreds of milliseconds,
after publication and routing already report the route gone. The scenario
therefore checks store stats on every poll of each generation, for both the
removal (one generation below the initial stats) and the recreation, rather
than sampling the removal stats once. A 2026-11-06 full run that sampled once
failed with only the HTTP `listener_count` unchanged; the gateway log showed
the HTTPRoute delete at `.660`, the Gateway delete at `.669`, and the full HTTP
rebuild reporting `removed_stale_listeners=3` about 0.6 s later. This was a
test sampling race, not a registry leak. A real leak still fails at the
20-second deadline.
Retained-handle unit tests cover those exact branches. TLS and UDP have no
separate wire case for this regression: TLS adds independent certificate and
SNI selection, while an established UDP session keeps its selected upstream;
the relevant deletion effect is on new sessions. The scenario does not claim
that an original listener resumes routing after Gateway recreation. That
workflow remains outside the [supported lifecycle](../intentional-design/gateway-recreation-requires-restart.md).

## Verification and re-evaluation

On macOS, after rebuilding `test_client` and `cache_residue`, the targeted
`run_integration.sh --no-prepare -r Gateway -i RetainedListenerHandles` run
passed the dynamic scenario and final cache-residue gate on 2026-09-23.
Workspace format, check, clippy, agent-doc, SSA, metrics inventory, Pingora
source policy, and Gateway API contract checks passed locally. Independent
read-only review found no regression in the runner or cleanup changes.

Linux CI did not execute this shared-topology scenario; the user explicitly
chose to close the task without that result. Re-evaluate the coverage if the
listener ownership model, manager removal order, integration runner dispatch,
or Linux CI execution contract changes. A Linux-only failure remains
unverified by the macOS result.

## Resolution references

- `route-manager-retained-listener-handle-integration` — cross-process coverage
  added; macOS targeted run passed; Linux CI verification waived for closure
  on 2026-09-23.
