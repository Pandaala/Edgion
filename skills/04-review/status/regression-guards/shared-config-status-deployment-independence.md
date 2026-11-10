---
name: shared-config-status-deployment-independence
description: Shared EGC status excludes deployment-local visibility; the owned GatewayClass reports early attachment errors and unresolved EGC references.
knowledge_type: regression-guards
---

# Shared configuration status is deployment-independent

## Conclusion and scope

EdgionGatewayConfig is cluster-scoped and can be observed by separate deployments
with different namespace visibility. Its `Accepted` condition reflects intrinsic
validation only. Early RequestId validation of `globalPluginsRef` belongs to the
consuming, deployment-owned GatewayClass. An invalid visible early attachment gives
`Accepted=False/InvalidParameters` there. Reference availability of the EGC (TLS
material, global plugin targets, trusted IpLists) is likewise reported only on that
class, as `ResolvedRefs=False/InvalidParameters` (see ctrl-proc2-02 below).

## Trigger and correction

Previously EGC.preparse inspected locally visible EdgionPlugins. One deployment
could reject an invalid early RequestId source while another could not see the
plugin and accept the same EGC. Alternating status writes/WATCH events then
oscillated Accepted indefinitely. A resourceVersion precondition prevents stale
writes, not two live writers continually computing different desired statuses.

The original shared `validate_request_id_early_attachment` call moved into
GatewayClass `resolve_params_ref`. It reads raw EGC and plugin generations, so
the plugins → EGC → GatewayClass requeue chain does not depend on cross-worker
processed-cache publication order. EGC retains plugin reference registration and
delete cleanup. Class ownership uses controllerName for reporting and publication.
Every matching class performs its own local parameter validation, including when
several classes share one deployment. The [reporting and publication contract](gatewayclass-reporting-and-publication.md)
preserves distinct identities for independent deployments. This does not add a second status writer.

## Limits and regression evidence

- A missing plugin (EdgionPlugins watched) makes the class `ResolvedRefs=False`
  but never `Accepted=False`; an unwatched plugin kind asserts nothing.
- (Original CAND-P3-02 correction) Intrinsic EGC validation, Gateway-side
  validation, schemas, and CA material resolution were unchanged. Independent controllers intentionally claiming the
  same class remain a deployment-ownership problem, not something CAS solves.
- `tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/gateway_class_test/status_tests.rs` alternates a shared EGC between
  two independent runtimes (invalid visible plugin versus absent plugin), asserting
  no shared status changes after the first reconciliation and independent class
  diagnostics. It also drives the actual plugin → EGC → class queues across
  traceId → ctx → traceId, deletion, and recreation, followed by status echoes.
- `global_request_id_validation_observes_replaced_raw_generation` replaces the
  former EGC-owned test with the same current-raw-generation coverage at the new
  owner and additional dependency-chain assertions. No integration tests were removed.
- These are in-process processor/queue tests, not live Kubernetes or deployment
  tests.

Historical targeted verification for the original correction:

- `cargo test -p edgion-controller --lib gateway_class -- --test-threads=2`: 10 passed.
- `cargo test -p edgion-controller --lib edgion_gateway_config -- --test-threads=2`: 12 passed.
- `cargo check -p edgion-controller --all-targets`: passed.
- Changed Rust files formatted with rustfmt; `git diff --check` and
  `python3 cicd/checks/validate_agent_docs.py`: passed.
- Broader workspace checks belong to the encompassing status-loop fix batch.
- Independent implementation and knowledge review: LGTM.

## Re-evaluation

Revisit when adding a deployment-local dependency check to shared resource status,
changing GatewayClass ownership, moving reference reads off raw stores, or altering
the plugin dependency requeue graph. See the [EGC resource guide](../../../01-architecture/05-resources/03-edgion-gateway-config.md)
and [GatewayClass resource guide](../../../01-architecture/05-resources/02-gateway-class.md).

## ctrl-proc2-02 EGC reference availability on the owning class

Before the fix, a missing outboundTls CA/client Secret, a missing global plugin, or a
missing trusted IpList left both EGC `Accepted=True` and GatewayClass
`ResolvedRefs=True`. Meanwhile the Gateway failed closed for TLS
(`OutboundTlsCaMissing`/`OutboundTlsClientCertInvalid`; one missing CA among several
fails the whole CA axis through `validate_ca_pairs` cardinality, it does not keep the
remaining bundles) and for global plugins (500 after gw-core-02), and silently
degraded RealIp (one unavailable trusted list disables all trust, so RemoteIp becomes
the direct peer). The suggested fix (EGC `ResolvedRefs` or `Accepted=False`) was
rejected: it would reintroduce the CAND-P3-02 multi-writer oscillation. Instead
`resolve_params_ref` collects, from this deployment's stores:
`egc_tls::unavailable_material` (read-only; registers no dependency, skips intrinsic
errors), `PluginTarget::Missing` global targets when EdgionPlugins is watched, and
`resolve_ip_reference` errors for trusted RealIp lists (InvalidReference skipped;
wrong-type/disabled lists are named "not an enabled IpList", not "not found").
Any entry gives `ResolvedRefs=False/InvalidParameters`; `Accepted` is unchanged
(the EGC exists and is well formed; its dependencies are reference resolution, and
Edgion Gateway eligibility does not depend on class conditions). Local absence is a
valid class signal because a deployment cannot ship material it cannot see: the
Gateway has no independent Secret path. Recovery reuses the existing
Secret/ConfigMap/IpList/plugin → EGC → class requeue edges.

### Limits

- The signal means "this class's EGC declares a reference this deployment cannot
  resolve", not "exactly this class's Gateways are affected". Global plugins and
  RealIp are class-scoped (bound from the class's parametersRef EGC), but the
  Gateway aggregates the outbound TLS default across all shipped EGCs
  (`GlobalOutboundTlsSnapshot::from_configs`, one declaring EGC per axis). A CA
  conflict falls back to system roots; a client-identity conflict fails every
  inheriting consumer. So a missing CA on one EGC can affect other classes, a class
  may report missing CA material that a conflict keeps out of use, and an EGC no
  owned class references can break inherited egress with no class signal at all.
- `dnsResolver.linkSysRef` is not checked; a dangling ref silently falls back to
  system DNS and is also aggregated across EGCs rather than class-scoped.
- A present Secret lacking the expected keys still counts as resolved (key and
  certificate parsing stays with outbound TLS consumers, as before).
- Secret/ConfigMap/EdgionConfigData kinds that are unwatched report their refs as
  unresolved (nothing can be shipped); an unwatched EdgionPlugins kind asserts
  nothing even though the Gateway still fails those globals closed.
- Kind init has no cross-kind barrier, so after a restart a class can be evaluated
  before Secret/ConfigMap/EdgionConfigData/EdgionPlugins stores fill and write one
  transient `ResolvedRefs=False`; dependency `on_change` and post-cache-ready
  revalidation requeue EGC → class and it converges.
- The default `kubectl get gatewayclass` column shows only `Accepted`; Gateway
  status does not mirror the class condition.

### Verification

`class_reports_locally_unresolved_global_references_and_recovers` covers all three
axes, a ConfigMap without CA data, recovery, the regression direction (a Secret
removed after resolving), a wrong-type IpList message, and unchanged shared EGC
status. The updated `gateway_class_test.rs` field-outcome matrix and status suites
cover watched/unwatched plugins. The `GcParamsFlap` dynamic integration scenario
gained an `unresolved` phase (missing `globalPluginsRef`, chosen over a CA ref so the
shared test Gateway never adopts a broken global TLS default; not run in this
change). `cargo test -p edgion-controller --lib -- gateway_class edgion_gateway_config`:
49 passed; `cargo check --workspace --all-targets` and clippy on changed files: clean.

### Re-evaluation

Re-evaluate if EGC gains another reference field or `dnsResolver.linkSysRef` must be
covered (add it to the class check), if the outbound TLS default becomes
class-scoped (tighten the first limit), or if a Gateway-side signal makes the class
condition redundant.

## Resolution references

- `ctrl-proc2-02` — EGC status hid unresolved references; reported on owning GatewayClass.
- `CAND-P3-02` — namespace visibility caused competing shared EGC status writes.

### ARCH-028 RealIp reference ownership

HTTP RealIp and global EGC RealIp store only nonempty `trustedIpRefs`; IpGroups live in referenced IpLists. The actual owning EP/EGC catalogue provides the same cold key compiler as Boolean IP conditions. A global EGC requires explicit target namespaces and permits only current Cluster visibility or the controller's validation-disabled scope; its TLS administrative exemption and namespaced grants do not apply. Controller generation processing clears old proof and dependency edges before recomputing from actual fields, including missing-target recovery edges. Shared EGC Accepted remains intrinsic-only; deployment-local trust availability adds no shared condition.

The global policy captures a ConfigData snapshot directly before PluginSession exists; the HTTP policy reuses its dispatch-local adapter view. Both preflight every trust ref once and use that same view for the PROXY-effective downstream peer and all forwarded hops. Any unavailable list makes all trust false for that decision; content/visibility/delete/recovery changes remain live without rebuilding policy or copying CIDRs. Invalid input publishes a sealed plugin generation or disables global extraction, with clean-generation recovery. The narrow realIp CRD preservation and typed invalid marker prevent removed/unknown fields from becoming silently pruned successful configs; resolver-only proof is schema-hidden and recomputed on ingest. Referenced IpLists retain central partial-good entry compilation, replacing the former inline strict-trust admission.
