---
name: backend-ref-selector-generation-ownership
description: BackendRef selector construction, failure states and WRR cursor lifetime belong to the compiled route generation.
knowledge_type: regression-guards
---

# BackendRef selector generation ownership

## Trigger and correction

ARCH-016 identified an unused publication seam: BackendSelector held private
SelectorState behind ArcSwapOption and exposed new/init/is_initialized even though
all production builders compiled before publishing the route generation. This was
structural debt, not evidence of a request race. BackendSelector now captures a
plain private state with new(items, weights); no initialization, default sentinel
or in-place replacement API remains. Requests select directly from that plan.

## Owners and preserved contracts

HttpRouteRuleUnit and GrpcRouteRuleUnit builders retain a single outer selector Arc
per rule across matches and hostname indexes. CompiledL4Backends owns one selector
inside its first-rule generation. WRR's atomic cursor remains shared across those
consumers. A newly compiled generation has an independent cursor; an unchanged unit
reused by an outer snapshot, or retained by an in-flight caller, keeps its current
cursor. Outer route publication owns hot reload; endpoint selection/health, retry,
persistence and AI dispatch retain their existing owners.

Empty input returns ERR_NO_BACKEND_REFS. Length mismatch returns
ERR_WRR_BUILD_FAILED before emptiness/weight validation. Mixed optional weights
return ERR_INCONSISTENT_WEIGHT. All-None weights default to one; nonpositive
weights are filtered; a single remaining slot uses the direct fast path. WRR
normalization and fallible construction remain canonical; TooManyItems maps to
ERR_WRR_BUILD_FAILED. HTTP/gRPC builders continue defaulting omitted weights to
one, while L4 retains raw optional weights and their validation semantics.

Raw slot indices and invalid/denied weighted shares remain intact. In particular,
HTTP is_generic_selectable includes Service, EdgionBackend and Invalid; this
refactor does not newly exclude AI slots. Ordinary HTTP rejects selected AI slots
at its existing peer boundary. Authorization and resolution stay post-selection.

## Evidence and verification limits

Production: lb/backend_selector/selector.rs, routes/http/match_unit.rs,
routes/grpc/match_unit.rs and backends/runtime.rs under edgion-gateway/src/.
Constructor/error/distribution regressions live in
edgion-gateway/tests/unit/lb/backend_selector/mod_test.rs. Existing HTTP fanout and
gRPC compiled-rule tests exercise shared cursor advancement, independent rebuilt
generations and retained old units. L4 runtime tests cover empty/partial-weight
errors, default weights and retained-generation cursor ownership for TCP/TLS/UDP.
Existing raw-slot/denied-share and WRR normalization tests remain applicable.

These checks verify ownership and behavior; they do not measure throughput or
assert a new benchmark result. Static API/caller inspection establishes that the
unused selector publication seam is gone. The ARCH-016 implementation passed
independent design and final review, workspace check/Clippy, formatting, repository
guards and 126 targeted tests covering backend_selector, HTTP/gRPC conf_handler_impl
and match_unit, and backends::runtime. These are completion evidence from this
implementation, not fresh verification of later changes.

## Re-evaluation triggers

Revisit when backendRef policies require actual in-place updates, construction
errors change, a builder compacts raw slots or weighted failures, or route reuse
changes cursor sharing. Do not restore a lifecycle for mocks, introduce an inner
swap without a real publication owner, or move endpoint health into this plan.

Authority: [load-balancing architecture](../../../01-architecture/02-gateway/06-load-balancing.md),
[WRR constructor errors](wrr-new-result-api.md) and
[weighted failure shares](../../routing/regression-guards/weighted-invalid-backendrefs-retain-traffic-share.md).
