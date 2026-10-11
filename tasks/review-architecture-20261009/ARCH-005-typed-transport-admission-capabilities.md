---
id: ARCH-005
status: candidate
kind: architecture-debt
priority: P2
primary_partition: GW-DISPATCH
analyst: /root/analyze_adapter
independent_verification: confirmed
---

# Publish transport admission capabilities from typed source entries

## Current ownership and evidence

`plugins/runtime/plugin_runtime.rs:464` identifies unary-response-incompatible
body filters by names BandwidthLimit/Wasm; `:504` identifies AI conversion owners
by request-filter names AiProxy/AiGuard/Guardrail. Resources expresses that same AI
conflict category independently through EdgionPlugin variants in
`resources/edgion_plugins/validate.rs:141`. EntryBundle caches executable projections
and other source metadata without these transport admission facts.

Filter name is documented as identity, but now determines conversion admission.
Wrappers forward the name and Wasm uses one identity across different stages, so
stage-sensitive meaning requires more than recognizing a plugin name.

## Call chains and change scenario

Plugin generations compile/cache entries into effective PluginRuntime views.
The HTTP request prepass checks conversion ownership before execution. Transcoder
installation checks ctx unary compatibility, which consults these runtime predicates.
Selected-backend admission also checks the actual request-pinned ExtensionRef runtime.

Adding a response consumer requires exhaustive factory/stage edits but unary
compatibility is a separate non-exhaustive string list. Adding an AI conversion
category requires typed Resources admission and an independent Gateway name list.
Factory exhaustiveness does not force either admission inventory to be reconsidered.

Observed cost is two AI category representations, request-time name scans and
tracing typed source/projection/wrapper/scope to establish semantic capabilities.
Omission/name-change divergence is a projected cost; no current bad admission or
measured throughput loss is claimed. Existing small inventories are readable and
currently agree with intended consumers.

## Bounded recommendation and legitimate complexity

Publish only these two transport facts during cold construction, using explicit
typed classification. Reuse an AI-owner classification between Resources validation
and Gateway preparation, retain stage-sensitive facts in existing EntryBundle,
aggregate into effective runtimes and read immutable facts at admission.

Preserve deliberate exhaustive factory matches, stage legality and failure policy.
Avoid a registry, universal plugin-kind framework, trait default/forwarding burden
or constructor redesign. No new request allocation is needed.

An ExtensionRef body trait does not prove its target consumes bodies. Wasm conflicts
only in the response-body stage; ExtProc modes and installed processor checks remain
dynamic. Native WAF's accepted empty phase-four window remains unchanged. Policy
masks, successful construction, immutable generations and actual selected backend
references determine effective membership; metadata must follow those exact views.

## Existing work, benefits and risks

Completed `waf-body-inspection-policy` work publishes conservative
`requires_complete_request_body` metadata through PluginRuntime and EntryBundle,
explicitly avoiding names as proof. Reuse that existing carrier rather than
introducing a parallel registry. Its complete/prefix request policy differs from
the two conversion admission facts here; no new complete-consumer implementation
is requested by this finding.

Expected benefit is explicit ownership of semantic classification and fewer
independent inventories/scans. Do not claim a speedup. The chief risk is publishing
metadata from raw/disabled/masked entries rather than effective executable views,
or incorrectly replacing dynamic ExtProc/pinned-target checks with static flags.

## Invariants and future verification

Preserve Block-before-body-work, disabled/Bypass inertness, source-owned cache,
companions/sharing/release, retained-generation restrictions, condition-conservative
admission, selected-only backend execution and sticky conflicts. Check typed/static
and compiled AI classifications, body/header-only Wasm/DSL/ExtensionRefs, allowed/
masked/allowed cache orderings, global/Gateway/rule/selected-backend matrices,
ExtProc modes and both installation orders, in-flight pins and demand release.
No new tracing, metrics, payload logs, response cache or full-response replay.
Existing tests were read; no build/test/benchmark was run.

## Prior records and independent disposition

Original exhaustive dispatch and builder tradeoffs, scoped policy publication,
key-scoped retention, transcoder admission/outcomes, scoped body dispatch, dye
parity, async phase boundaries and request-template unification were read. This
retains their boundaries. Wiring-plan and WAF policy records were searched/read;
current completed composition metadata does not publish these two conversion
transport facts. Independent review confirmed the bounded classification debt
while retaining effective-view publication and the existing metadata carrier.
No functional defect is asserted.
