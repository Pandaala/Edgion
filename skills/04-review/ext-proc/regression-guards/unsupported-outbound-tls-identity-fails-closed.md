---
name: unsupported-outbound-tls-identity-fails-closed
description: Use when reviewing ExtProc effective outbound TLS identity capabilities and constructor rejection.
knowledge_type: regression-guards
---

# Unsupported outbound TLS identity policies fail closed

## Conclusion and scope

For TLS-enabled ExtProc clients, any effective `validation.hostname` or nonempty
`validation.subjectAltNames` rejects construction. ExtProc does not implement
hostname override or SAN pinning. Absent and empty SAN lists remain supported.
This is configuration rejection, independent of the runtime `failureModeAllow`
dependency policy.

## Rationale and evidence

The [outbound TLS contract](../../../02-features/03-resources/18-link-sys.md)
identifies these ExtProc identity features as unsupported. Controller
`handlers/san_guard.rs::check_plugins_san` reports local unsupported fields through
status, but lenient admission still ships the resource to the Gateway.

In `edgion-gateway/src/plugins/http/ext_proc/client.rs`,
`ExtProcClient::new` first calls canonical `resolve_outbound_tls`, then
`validate_effective_tls_capabilities`, before fingerprinting, target-cache lookup,
or channel construction. The check examines `ResolvedOutboundTls.san`, preserving
global inheritance and local whole-block replacement. The shared resolver may
force `verify: true` for a SAN policy; that does not implement SAN matching, so the
capability check still rejects it. Static errors name the unsupported field and
remedy without disclosing configured identity values.

`ExtProc::create_attached` declines construction on this error. `compile_plugins`
records the invalid plugin and seals its runtime with `RejectInvalidConfig`.
The compiled resource is not activatable and does not request last-known-good
retention. The existing reject filter returns HTTP 500 with the generic body
`{"message":"plugin configuration error"}`; no new request logging is needed.

## Alternatives and limits

Do not silently ignore the policy, treat ordinary chain verification as SAN
pinning, or implement a separate TLS resolver. Supporting hostname override or SAN
pinning requires a separately scoped transport change and handshake evidence.
This guard preserves the existing CA replacement, client identity, skip-verify,
egress, cache, and streaming behavior. TLS-disabled configurations do not enter
the effective TLS capability check.

## Verification and re-evaluation

Focused tests in `client.rs` cover:

- `unsupported_tls_identity_is_rejected_during_construction`: hostname presence
  (including empty), SAN, both fields, and SAN combined with skip-verification.
- `effective_tls_identity_guard_respects_inheritance_and_whole_block_override`:
  explicit defaults, inherited rejection, local replacement, and empty SAN lists.
- `supported_effective_tls_policies_build_without_global_state_changes`: system
  and custom CA configuration, mTLS identity, skip-verification, and h2 ALPN.
- `unsupported_tls_identity_compilation_seals_generic_rejection_for_both_failure_modes`:
  in-memory compilation and execution of the first sealed filter.

Run each with `cargo test -p edgion-gateway --lib
plugins::http::ext_proc::client::tests::<name> -- --exact`.
All four passed during B0283 verification, together with seven exact existing
non-network phase, response-status, timeout, unsolicited-verdict, and half-close
tests. The initial test build failed because its new rcgen fixture referenced
`key_pair`; the locked API uses `signing_key`. After correcting that fixture,
the original exact test and the remaining selected tests passed.

Builder success is not TLS handshake evidence. The unit tests above use explicit
defaults or local policies and do not establish live publication, absence of
network activity, or recovery. Their historical results do not substitute for
running the dedicated integration scenario.

The `ExtProcTlsPolicyRecovery` dynamic scenario addresses task
`add-ext-proc-unsupported-tls-policy-recovery-integration`. It uses the shared
Controller/Gateway topology and the existing full-tier independent Go processor.
Both failure modes exercise local hostname/SAN rejection, supported-policy
recovery, inherited hostname/SAN rejection, whole-block local replacement, and
supported inherited-policy recovery. It separately checks Controller advisory
status, the published plugin phase and policy, generic HTTP 500 rejection, and
successful TLS request/response header processing. Raw ConfigClient values are
not a runtime publication acknowledgement: the scenario also waits for new
per-resource IDs in `plugin_publication_inventory.http_generations`, route
inventory/table counts, and actual EdgionGatewayConfig store counts. For the
global policy, deleting/recreating the dedicated config provides an observable
count transition after `recompute_global_config`; it does not restart processes.
The scenario owns a narrowly scoped `ReferenceGrant` from the cluster-scoped
EdgionGatewayConfig resolver identity to the fixture CA Secret. Before publishing
an inheriting plugin generation, it also requires the Gateway admin view to expose
the resolved CA field as redacted. This prevents a missing CA from producing the
same generic rejection while bypassing the ExtProc capability guard under test.

The fixture's cumulative TCP accept counter increments before TLS handshakes;
its RPC counter counts calls even over reused connections and without test
headers. Invalid-generation assertions compare both counters across plugin
publication (including global changes) and traffic, alongside absent per-case processor observations. A
successful phase waits for a completed TLS stream. The scenario restores global
defaults by deleting its own config and polls for raw/runtime removal of its own resources
on success and failure. Existing static scenarios remain intact.

Scope limits: the global aggregator currently adopts a validation block from a
unique CA-declaring config, so the fixture explicitly supplies a CA reference.
This is not evidence about identity-only global aggregation. Each inherited
phase publishes a new plugin entry after its global resource arrives; it does
not establish global-only invalidation of an already compiled plugin. Endpoint
counters require serial shared-topology execution. The external fixture stays
in the full tier; fast dynamic sweeps report an explicit skip.

Fresh closure verification on 2026-09-12 ran the full targeted command from the
scenario's
[run contract](../../../../edgion-tests/integration/conf/EdgionPlugins/ExtProc/DynamicTest/README.md).
The first run exposed the missing ReferenceGrant because the final supported
inherited policy remained sealed with an unresolved CA. After adding the owned
grant and resolved-CA readiness assertion, both `EdgionPlugins_ExtProc` and
`Dyn_ExtProcTlsPolicyRecovery` passed, followed by the runner's unknown-field,
requeue-loop, metrics-label, and cache-residue checks. Independent review then
found that the newly exercised cross-namespace CA dependency was not cleared on
EdgionGatewayConfig update or deletion; after adding symmetric cleanup and a
focused replacement/deletion unit test, the full targeted command passed again.

Re-evaluate this guard when ExtProc implements either identity capability or when
canonical TLS precedence, constructor failure handling, or publication policy
changes. Historical phase-verdict pages remain separate knowledge.
