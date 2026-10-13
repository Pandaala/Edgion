---
name: shared-config-status-deployment-independence
description: Shared EGC status excludes deployment-local plugin visibility; the owned GatewayClass reports early attachment errors.
knowledge_type: regression-guards
---

# Shared configuration status is deployment-independent

## Conclusion and scope

EdgionGatewayConfig is cluster-scoped and can be observed by separate deployments
with different namespace visibility. Its `Accepted` condition reflects intrinsic
validation only. Early RequestId validation of `globalPluginsRef` belongs to the
consuming, deployment-owned GatewayClass. The consuming class keeps `ResolvedRefs=True`
when the EGC exists; an invalid visible early attachment gives `Accepted=False/InvalidParameters`.

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

- Missing/unwatched plugins preserve the existing unknown/not-rejected policy;
  class acceptance here is not a claim that an unseen plugin is usable.
- Intrinsic EGC validation, Gateway-side validation, schemas, and CA material
  resolution are unchanged. Independent controllers intentionally claiming the
  same class remain a deployment-ownership problem, not something CAS solves.
- `handlers/tests/gateway_class_status_test.rs` alternates a shared EGC between
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

## Resolution references

- `CAND-P3-02` — namespace visibility caused competing shared EGC status writes.

### ARCH-028 RealIp reference ownership

HTTP RealIp and global EGC RealIp store only nonempty `trustedIpRefs`; IpGroups live in referenced IpLists. The actual owning EP/EGC catalogue provides the same cold key compiler as Boolean IP conditions. A global EGC requires explicit target namespaces and permits only current Cluster visibility or the controller's validation-disabled scope; its TLS administrative exemption and namespaced grants do not apply. Controller generation processing clears old proof and dependency edges before recomputing from actual fields, including missing-target recovery edges. Shared EGC Accepted remains intrinsic-only; deployment-local trust availability adds no shared condition.

The global policy captures a ConfigData snapshot directly before PluginSession exists; the HTTP policy reuses its dispatch-local adapter view. Both preflight every trust ref once and use that same view for the PROXY-effective downstream peer and all forwarded hops. Any unavailable list makes all trust false for that decision; content/visibility/delete/recovery changes remain live without rebuilding policy or copying CIDRs. Invalid input publishes a sealed plugin generation or disables global extraction, with clean-generation recovery. The narrow realIp CRD preservation and typed invalid marker prevent removed/unknown fields from becoming silently pruned successful configs; resolver-only proof is schema-hidden and recomputed on ingest. Referenced IpLists retain central partial-good entry compilation, replacing the former inline strict-trust admission.
