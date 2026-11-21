---
name: resource-edgion-gateway-config
description: EdgionGatewayConfig resource — Edgion's extended global gateway configuration.
---

# EdgionGatewayConfig resource

## Unified downstream TLS policy

`ResolvedGatewayConfig.downstream_tls` owns `DownstreamTlsPolicy`: the EGC
minimum, shared cold `CipherPolicy`, and existing numeric `CurvePolicy`.
`CompiledEdgionTls` uses the same cipher compiler. Absent ciphers inherit;
explicit `[]` means backend defaults; a rendered CString or rejection remains
immutable. Resources/Controller validate NUL and curve syntax without linking
BoringSSL. The strict backend cipher setter validates effective expressions.

The early callback first selects the native, EdgionTls or exact frontend-mTLS
overlay identity, then applies policy once through `apply_selected_downstream_policy`.
Local minimum/ciphers replace EGC defaults independently, including a lower local
minimum or a valid local cipher masking an invalid default. Curves have no local
override. Unspecified ciphers inherit active FIPS/backend defaults; an explicit
empty list bypasses only the cipher default, never FIPS curve restrictions.
Any effective-policy error emits one canonical SSL audit and returns select error.
Certificate callbacks configure identity/mTLS only and never reload/reapply policy.
The local EGC Arc lives through application; SSL owns the copied settings afterward,
so no redundant EGC Arc is stored in SslCtx. Identity Arcs retain their existing lifetime.

No Rust handshake cipher joining, parsing or temporary cipher allocation occurs.
BoringSSL's public strict cipher setter internally parses its string; this existing
backend cost remains accepted. No SSL_CTX swapping, new locks or stores are added.
TLS1.3 suites and physical listener ownership/retargeting boundaries remain unchanged.


> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

EdgionGatewayConfig is a cluster-scoped Edgion-extension resource used to configure global gateway behavior parameters. It is a custom extension outside the Gateway API standard.

## Source code locations

- Controller Handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_gateway_config.rs`
- Gateway ConfHandler: `edgion-gateway/src/config/edgion_gateway/conf_handler_impl.rs`
- Type definitions: `edgion-resources/src/resources/edgion_gateway_config.rs`

## Controller-side processing

### parse

`preparse` rejects unsupported outbound TLS SAN pinning, validates
`spec.retry.attempts` against the shared route domain, and validates that DNS resolver
`linkSysRef` and inline servers are mutually exclusive. It also validates
`spec.pluginPolicy` with the shared pure compiler; invalid names, duplicate
entries, action/status combinations and out-of-range statuses surface as
source validation errors.

The shared cluster-scoped `Accepted` condition depends only on intrinsic config
validation, not on which namespaces a deployment can observe. Early RequestId
validation of referenced `globalPluginsRef` resources runs on the consuming
GatewayClass and reports `Accepted=False/InvalidParameters` there. Reference
availability (TLS material, global plugin targets, trusted IpLists) is reported
only as the consuming class's deployment-local `ResolvedRefs`, never on shared EGC.
Plugin dependency registration stays on EGC, preserving plugin → EGC → class
requeues. See the [shared status guard](../../04-review/status/regression-guards/shared-config-status-deployment-independence.md).

The actual-owner `PreparedEgcTlsReferences` visitor validates a true Cluster owner
and explicit target namespaces. `preparse` reports intrinsic reference errors,
including an invalid owner even when no TLS block exists. Material visibility does
not enter shared EGC Accepted. No shared ResolvedRefs condition is introduced.

`parse` clears prior registrations and both resolved material fields before
reading stores. The two exact fields `outboundTls.validation.caCertificateRefs[]`
and `outboundTls.clientCertificateRef` use opaque administrative selections from
their actual EGC owner. EGC write authority authorizes selecting named visible
material in explicit namespaces. These selections do not require ReferenceGrant
and do not fabricate a source namespace or Grant dependency. This authority does
not extend to namespaced plugin/LinkSys references or globalPluginsRef.
CA kinds are Secret/ConfigMap; the client kind defaults to Secret. Omitted/empty
group and the approved custom `core` spelling select the core API. Target namespace
must be explicit, nonempty and valid. Every valid slot, including missing material,
registers a recovery dependency; invalid slots create no lookup/edge. Canonical
dependency ownership is EGC/name/namespace=None, including malformed-namespace
updates and delete cleanup. CA and client outcomes remain independent.

Ordinary Secret copies lose the reserved ConfigMap marker; actual ConfigMaps use
the shared provenance wrapper. No requested identity is written over fetched
metadata. Material/key/certificate parsing remains with outbound TLS consumers.
The owning global policy loan borrows actual policy/material. Outbound TLS
consumers capture one published `GlobalOutboundTlsSnapshot` during preparation
instead of reading global policy later; that migration completed through R12b
(`6b5adb0ae`).

Change/delete events requeue all GatewayClass resources because their parametersRef status depends
on EdgionGatewayConfig existence; delete also clears Secret registrations.

### update_status

- `Accepted`: True (reason=Accepted) when there are no validation_errors; False (reason=Invalid) when errors exist

## Gateway-side processing

### Runtime EGC snapshot

The existing `EDGION_GATEWAY_CONFIG_STORE` publishes one
`ArcSwap<EdgionGatewayConfigSnapshot>`. Its resource map and
`HashMap<String, Arc<ResolvedGatewayConfig>>` advance together on full and partial
updates. `ResolvedGatewayConfig::resolve` parses durations, body limits and header
names, validates and constructs RealIp extraction, and resolves security/server
values on the configuration path. EGC remains an independently synchronized
resource; Controller does not embed it into Gateway resources.

`bind_gateway_config` resolves the selected name once during listener construction
and returns a shared `Arc<GatewayConfigHandle>`. Publication swaps that handle's
resolved generation. `GatewayConfigHandle::load` performs one atomic Arc load
without hashing a name or reading the resource map. HTTP `new_ctx` pins the result
for request metadata, upstream retries, compression and response processing.
The HTTP/1 keepalive budget is separately retained in Pingora persistent
connection context from the first request. TCP/TLS `process_new` and UDP session
creation capture their own runtime Arc; TLS certificate selection captures at
handshake start. Missing/deleted names and no reference use shared defaults.

This propagation is Edgion's chosen product contract, not a requirement that
Gateway API imposes on all GatewayClass parameter implementations. It does not
retarget a listener's EGC name on GatewayClass reference changes, rebind ports,
transfer shared-listener ownership, or enable Gateway deletion/recreation.
See [EGC publication regression boundaries](../../04-review/listeners/regression-guards/egc-runtime-snapshot-publication.md).

### Existing subsystem publishers

WAF captures `spec.waf.expensiveChecks` (default false) from this same runtime
snapshot. It gates compiler-classified canonical HPP and Multipart Content-Type
aggregate state/checks; ordinary JSON/XML parsing and other attacks remain active.
The Multipart collector keeps its capture/observer path. The leaf
engine's direct constructors default to original all-checks-on behavior, while
Gateway always supplies its pinned explicit policy. Unknown custom dependencies
conservatively keep the required family. See the user configuration reference for the
canonical TX namespace overlap boundary.

LinkSys, load-balancing defaults, outbound TLS and DNS keep their existing global
dynamic consumers. Plugin policy, global references and access-log masking keep
their scoped publisher. These publications are not an atomic transaction with
the request's runtime EGC Arc.

The plugin-policy publisher resolves `spec.pluginPolicy` only through each
GatewayClass parametersRef. Unreferenced EGC resources do not contribute policy;
classes sharing one EGC share its policy while retaining their own attachment
scope. Policy, global plugin references, source generations and attachments
produce one immutable scope. EGC/class/binding changes rebuild affected views
without restarting listeners. Missing/invalid referenced configuration denies
new admission; no parametersRef selects the explicit default scope.

Missing plugin policy scope still denies admission even though ordinary runtime
EGC lookup falls back to defaults. See
[plugin publication](../../04-review/plugin-runtime/regression-guards/scoped-policy-runtime-publication.md).

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| EdgionGatewayConfig → GatewayClass → Gateway | GatewayClass parametersRef | scoped publication | Plugin execution policy affects only bound classes and their Gateways |

### Global DNS candidate ownership

The DNS publisher selects the minimum raw store key with a DNS field before
validating that winning actual EGC reference. Its explicit LinkSys object target
requires valid namespace/name and a true cluster-scoped owner. Invalid references
retain the winning snapshot with no candidate (System), rather than selecting a
later EGC or same-field inline servers. Inline-only servers and independently
parsed TTL preserve their existing behavior, including zero TTL and invalid-TTL
defaulting. Loaded DNS Arcs retain candidate and TTL across update/removal; live
HTTPDNS provider lookup remains separate and follows the current typed store.

### Actual HTTP attachment field binding (C1)

`globalPluginsRef` uses the shared `HttpPluginAttachments::EgcGlobals` actual-field adapter before dependency registration or plugin classification. The actual EGC must have a valid cluster identity (no metadata namespace); an omitted target namespace means `default`, while an explicit empty namespace is invalid. Ordered valid entries, including duplicates, remain intact. Invalid entries have no dependency or lookup. Intrinsic errors do not depend on EdgionPlugins watch visibility. GatewayClass skips when no EGC processor exists; once the EGC is acquired it checks intrinsic identity even when plugins are unwatched. A valid global target that is missing while EdgionPlugins is watched makes the consuming class `ResolvedRefs=False` (never `Accepted=False`); an unwatched plugin kind asserts nothing.

### ARCH-028 RealIp reference ownership

HTTP RealIp and global EGC RealIp store only nonempty `trustedIpRefs`; IpGroups live in referenced IpLists. The actual owning EP/EGC catalogue provides the same cold key compiler as Boolean IP conditions. A global EGC requires explicit target namespaces and permits only current Cluster visibility or the controller's validation-disabled scope; its TLS administrative exemption and namespaced grants do not apply. Controller generation processing clears old proof and dependency edges before recomputing from actual fields, including missing-target recovery edges. Shared EGC Accepted remains intrinsic-only; deployment-local trust availability adds no shared condition.

The global policy captures a ConfigData snapshot directly before PluginSession exists; the HTTP policy reuses its dispatch-local adapter view. Both preflight every trust ref once and use that same view for the PROXY-effective downstream peer and all forwarded hops. Any unavailable list makes all trust false for that decision; content/visibility/delete/recovery changes remain live without rebuilding policy or copying CIDRs. Invalid input publishes a sealed plugin generation or disables global extraction, with clean-generation recovery. The narrow realIp CRD preservation and typed invalid marker prevent removed/unknown fields from becoming silently pruned successful configs; resolver-only proof is schema-hidden and recomputed on ingest. Referenced IpLists retain central partial-good entry compilation, replacing the former inline strict-trust admission.

## Downstream ECDH policy

`spec.downstreamTls.ecdhCurves` uses the shared Resources validator in Controller
preparse and Gateway cold compilation. `ResolvedGatewayConfig` owns the compiled
numeric group list or sealed rejection state. The existing startup-bound handle
loads this generation once per handshake and applies it directly for every SNI
and certificate source. There is no listener override or policy matcher. Missing
EGC retains the existing default-generation behavior. Shared socket ownership and
GatewayClass retargeting limitations remain unchanged. See the
[TLS policy owner](../02-gateway/04-tls/SKILL.md).
