# Envoy filter design: lessons for Edgion

## Scope and evidence

Investigated on 2026-09-25. Envoy source is pinned to commit `6da46eead1112afa0f4b85b14e3e6ef7d0184761`, obtained from the official GitHub API (commit timestamp: 2026-09-25T04:05:20Z). The `latest` documentation is a development snapshot, not a claim about every released Envoy version. Source observations below are separate from proposed Edgion adaptations. No Envoy build or benchmark was run.

This supplements [the implementation plan](README.md). Edgion already implements the main local-construction and multi-hook ownership ideas. Configuration-time contract checks are a possible future topic. The current implementation scope is only reduced manual wiring; the revised README steps A-C supersede earlier P0-P5 references below.

## 1. Local factories plus explicit build selection

Envoy's `REGISTER_FACTORY` connects a concrete factory to a category registry. Its implementation supports a static-initializer mode and an explicit `forceRegister` function mode. The registry maps names/types to factories and rejects duplicate registrations. Built-in extensions also have an explicit name-to-Bazel-target build catalogue; adding a source file alone does not make a filter available. See the pinned [registry](https://github.com/envoyproxy/envoy/blob/6da46eead1112afa0f4b85b14e3e6ef7d0184761/envoy/registry/registry.h) and [extension build catalogue](https://github.com/envoyproxy/envoy/blob/6da46eead1112afa0f4b85b14e3e6ef7d0184761/source/extensions/extensions_build_config.bzl).

Edgion adaptation: keep centralized, exhaustive compile-time registration but let an entry name a small plugin-local construction function when it has real preparation logic. Do not spread the stage matrix across default trait methods. Directory scanning, inventory/linkme registration, and runtime string lookup do not follow from the locality benefit.

Value: the locality benefit is already present in plugin-owned constructors. Do not count it as a new saving. Moving a trivial one-line constructor into a separate factory file adds little value; avoid mandatory per-plugin factory boilerplate.

## 2. Typed adapters remove boilerplate before plugin-specific construction

Envoy's `CommonFactoryBase` handles typed config conversion/validation and common factory metadata. At the inspected commit, `UnifiedFactoryBase` provides a common entry point for upstream and downstream HTTP factories; several older factory bases are deprecated. These are upstream/downstream chain locations, not the decode/encode directions of an individual stream. See [factory_base.h](https://github.com/envoyproxy/envoy/blob/6da46eead1112afa0f4b85b14e3e6ef7d0184761/source/extensions/filters/http/common/factory_base.h).

Edgion adaptation: Resources continues to own typed schema and pure validation; Gateway owns executable construction. Any generated dispatch must preserve explicit variant-specific validation. Keep existing constructor arguments; no new common build context is needed for the narrowed task.

Re-evaluation against current code: this separation is already established. Resources owns the strict serde enum and `PluginValidation`; Gateway's `plugin_validation_error` delegates to it and adds resolved-state checks; `create_*_from_edgion` adapts typed configs to plugin-local `new`/`create`/`try_new`; `add_from_*` owns wrappers and failure accounting. CORS, OIDC, and FormJsonTransform constructors contain their own preparation logic. Different constructor inputs/results do not establish a need for a universal factory trait or context.

Value: confirmation of the existing architecture, with low incremental value from an additional factory layer. P1/P2 can still reduce mechanical declarations, but that is a maintenance change, not a missing factory abstraction. Rust does not need C++ inheritance or protobuf `Any` here.

## 3. One logical filter may install multiple hooks

Envoy's `FilterFactoryCb` installs filters through callbacks and may install more than one filter. `FilterChainFactoryCallbacks` offers decoder-only, encoder-only, and combined stream-filter installation. A combined filter participates in both directions. See [filter_factory.h](https://github.com/envoyproxy/envoy/blob/6da46eead1112afa0f4b85b14e3e6ef7d0184761/envoy/http/filter_factory.h) and [filter.h](https://github.com/envoyproxy/envoy/blob/6da46eead1112afa0f4b85b14e3e6ef7d0184761/envoy/http/filter.h).

Edgion already implements the core behavior: ExtProc/Guardrail return request and response companions from `create_attached`; the request builder applies their wrappers; `into_entry_outcome` groups their stage projections and failure state into `EntryBundle`; `EntryCache` caches that result by source slot; and `append_entry_bundle` reuses the Arcs in effective views. `PluginGeneration` bounds sharing to one immutable source and releases entries according to active demand.

The response implementations additionally require the request-installed attachment with the same identity before doing work. Existing `response_companions_share_and_release_with_their_request_entry` coverage checks lazy construction under bypass, pointer sharing across compiled views, and release after the last owner. This investigation read that test; it did not execute it.

The two explicit installation branches also expose real differences: ExtProc receives a resolved body gate, while Guardrail does not use that same wrapper. Both preserve request-owned conditions and companion identity. Replacing these branches with a new `ConstructedHooks` type would principally move this logic, while adding another construction representation beside the existing tuple and bundle.

Revised recommendation: retain the current attached construction and bundle architecture. The earlier claim of high incremental value and the proposed P2 attached-hook abstraction spike were overstated and are withdrawn. Reconsider only if further concrete companion plugins repeat the same installation contract, or an actual drift defect identifies the exact common operation to extract. No present architecture defect or performance improvement has been established.

## 4. Separate prepared configuration from stream state

The header-to-metadata example calls `Config::create` while preparing the factory, captures the shared result, and creates the stream filter when its callback runs. See [the concrete factory](https://github.com/envoyproxy/envoy/blob/6da46eead1112afa0f4b85b14e3e6ef7d0184761/source/extensions/filters/http/header_to_metadata/config.cc).

Edgion adaptation: retain its existing generation-owned shared filter objects and request-owned mutable state. Compile regexes and other reusable structures during publication. Do not copy Envoy's per-stream filter allocation into Edgion's shared `Arc<dyn ...>` chains: object lifetime is a runtime design choice, not a registration requirement. Current constructors, entry caching, and attached request state already implement much of this separation.

Value: important architectural boundary and contributor guidance, but low incremental implementation value where Edgion already follows it. Do not count it as a new optimization delivered by this plan.

## 5. Declare dependencies and validate the configured sequence

Envoy defines required/provided headers, filter-state keys, and metadata. Its filter-chain helper checks registered factory dependencies while processing static filters. The inspected dependency manager validates decode dependencies only; encode fields in the protobuf do not establish encode enforcement. Dynamic discovery follows a separate path in the helper, so this is not proof of complete validation for every dynamic chain. See [dependency schema](https://github.com/envoyproxy/envoy/blob/6da46eead1112afa0f4b85b14e3e6ef7d0184761/api/envoy/extensions/filters/common/dependency/v3/dependency.proto), [helper](https://github.com/envoyproxy/envoy/blob/6da46eead1112afa0f4b85b14e3e6ef7d0184761/source/common/http/filter_chain_helper.h), and [dependency manager](https://github.com/envoyproxy/envoy/blob/6da46eead1112afa0f4b85b14e3e6ef7d0184761/source/common/http/dependency_manager.h).

Edgion has concrete related cases in `validate.rs`: AiGuard-before-AiProxy and RateLimitRedis overflow references to an earlier active RateLimitLocal alias. These are distinct contracts. AiProxy does not universally require AiGuard; a present guard must precede it. RateLimitRedis's dependency is conditional, instance-specific, and requires a particular producer configuration.

A later capability design could distinguish `before_if_present`, `requires_alias`, and typed produced facts. Do not auto-sort operator chains or convert optional ordering into mandatory dependencies. Conditions, disabled entries, class policy, cross-resource scopes, and whether a producer actually ran must remain explicit. Static checks do not replace runtime body-generation validation or missing-outcome behavior.

Value: medium-to-high as plugin interactions grow; not needed to complete mechanical P0-P3. Start with the two real contracts rather than designing a general dependency graph engine.

## 6. Explicit lifetime and type for shared state

Envoy distinguishes immutable typed metadata, prepared route config, and dynamic stream/connection state. Its typed metadata is converted during config load; filter state has explicit ownership/lifetime semantics. See [official data-sharing documentation](https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/advanced/data_sharing_between_filters).

Edgion can borrow the requirement to identify the owner and lifetime of every new plugin state value. Existing source-bound outcome slots and attached ExtProc/Guardrail state are relevant local precedents. A generic `HashMap<String, Box<dyn Any>>` is not automatically an improvement: it adds lookup/allocation costs and must distinguish two instances of the same plugin. Prefer existing typed slots for hot data; consider a bounded typed extension mechanism only after concrete repeated needs justify it.

Value: medium for reducing future context coupling, but no immediate rewrite is recommended.

## What not to import into the wiring task

- Envoy's decode order and reverse encode order are an explicit chain contract. Edgion has separately configured stages; changing order is a behavior change, not registration cleanup. See [HTTP filter ordering](https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/http/http_filters).
- StopIteration/continue and buffering form a filter-manager flow-control model. Edgion must preserve its own Pingora callbacks and canonical bounded streaming processor. Current Edgion does have async streaming processors; older notes describing only synchronous response-body hooks do not cover the whole current system. Do not add full-response aggregation.
- Per-route override semantics are a separate product/configuration feature. They do not reduce built-in plugin registration work.
- Envoy also has dynamic modules with an official Rust SDK. This demonstrates that the language is not a barrier, but dynamic shared-library ABI/loading is a different extension product. It is not necessary for reducing edits to in-repository plugins. See [official dynamic-module documentation](https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/advanced/dynamic_modules).

## Updated recommendation

Follow the revised README steps A-C: reuse names, generate CRD discriminators from existing schema/stage facts, and prototype a small centralized factory-dispatch declaration. Resources enum generation is deferred. Do not add `ConstructedHooks`, a universal factory trait/context, mandatory factory files, or generalized stage installation. Existing plugin-local constructors and source-bound bundles already provide the main architectural benefits discussed above. Keep stage legality and validation visible centrally. Dependency contracts remain outside the current scope.

The earlier 19-to-12 estimate included the deferred Resources rewrite and is superseded. The revised scope estimates 19-to-17 locations for A/B, or 19-to-14 if C passes its gate; shared manual files fall from approximately 8 to 7. These remain estimates for an extension exercise, not measured productivity gains. Envoy comparison does not prove additional savings. No production code was changed.
