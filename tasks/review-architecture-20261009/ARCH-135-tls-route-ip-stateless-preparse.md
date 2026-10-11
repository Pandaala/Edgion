---
id: ARCH-135
status: candidate
kind: architecture-debt
priority: P3
partition: PART-411
analyst: /root/verify_ip_union
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Reuse stateless validation for TLS-route IP restriction preparse

EdgionStreamPluginsHandler::preparse in edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_stream_plugins.rs uses PluginValidation::validation_error for Stage-1 L4 IpRestriction and ConnectionRateLimit. Its enabled Stage-2 TlsRouteStreamPlugin::IpRestriction arm instead calls config.clone().validate_and_init() and drops the resulting config. This arm carries the existing HTTP-layer IpRestrictionConfig, whose Resources implementation already exposes the stateless PluginValidation entry point.

IpRestrictionConfig::validate_and_init in edgion-resources/src/resources/edgion_plugins/plugin_configs/ip_restriction.rs:171-211 checks configured allow/deny intent and final HTTP status, constructs partial-good inline matchers, then checks reference-list structure. The Controller uses only its Result. For a valid enabled Stage-2 entry with inline CIDRs, preparse therefore copies the config, allocates and populates matcher-building state, stores the matchers on the temporary config and immediately discards them. The Gateway's TlsRouteIpRestriction::from_bound separately initializes the actual runtime config and retains those matchers. This is cold generation work, not a per-connection cost or a measured performance result.

The bounded correction is to call the existing config.validation_error() in this Controller arm and preserve the existing tlsRoutePlugins[index] (IpRestriction) diagnostic wrapper. No new validation helper, trait, cache, registry or interface is necessary. Runtime initialization remains in TlsRouteIpRestriction::from_bound because that owner needs the matcher side effects.

The shared stateless implementation has the same structural rejection predicates and ordering here: no configured lists, invalid final status, invalid allowRefs structure, then invalid denyRefs structure. Inline CIDR problems intentionally remain partial-good rather than becoming admission errors. build_grouped_matcher_partial returns a usable matcher and an error list rather than propagating matcher-build errors through validate_and_init; this Controller call discards that list. Existing status collection remains the owner of partial-good entry diagnostics.

The two APIs do not have byte-identical reference error text. validation_error qualifies an invalid reference as allowRefs: or denyRefs:, whereas validate_and_init currently returns the bare validate_refs_structure message. Adopt the existing canonical qualified diagnostic explicitly, retain the outer Stage-2 wrapper and first-error order, and cover this distinction in the Controller wiring tests. Do not claim an exact-text-preserving one-line change or strip these prefixes through a new adapter.

The structural benefit is removing data-plane initialization from an admission-only consumer and making Stage-2 use the existing validation contract already consumed elsewhere. It also avoids this discarded config copy and matcher preparation. The smaller alternative is retaining the current call; it requires no new mechanism but leaves the unnecessary preparation boundary. No request-path allocation, speed estimate or broad GCIR validation rewrite is proposed.

Preserve known-disabled entry skipping, unsupported-type rejection even when disabled, original entry ordinals, Accepted=False/Invalid for structural errors, ConfigData reference registration and authorization, partial-good status diagnostics, authored whitelist intent, deny-first evaluation, missing-reference behavior and Gateway fail-closed initialization. Preserve separate L4 and HTTP-layer schema contracts and the legitimate runtime matcher owners. No logging, metrics, body handling or observability expansion is needed.

The original stream-preparse-uses-plugin-validation regression guard documents the completed Stage-1 F009 correction and explicitly identifies Stage-2 as outside that correction. This is not a repeat of Stage-1 or a proposal to extract the deliberately separate runtime constructors. ARCH028 concerns hot-path borrowed matcher-union projections; ARCH056 concerns the legacy stream schema detector. Neither owns this admission-only call-site correction.

Actual existing Controller test bodies cover invalid Stage-2 structure, disabled Stage-2 skipping, partial-good inline errors, unsupported-type transport and reference recovery. Resources trait tests cover empty-list rejection, final-status bounds and intentional invalid-inline-CIDR tolerance. Future verification should exercise valid inline Stage-2 configs through stateless preparse, structural errors and their order, canonical allowRefs/denyRefs prefixes, disabled behavior and unchanged partial-good reporting. Use existing pure validation tests and narrow Controller wiring tests; do not add runtime matcher caches or allocations for mock convenience. No tests or builds were executed.
