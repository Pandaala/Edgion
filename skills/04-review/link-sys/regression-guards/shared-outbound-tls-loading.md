---
name: shared-outbound-tls-loading
description: Use when reviewing LinkSys TLS enablement, certificate resolution, readiness, and shared provider loading.
knowledge_type: regression-guards
---

# LinkSys outbound TLS enablement and shared loading

## Conclusion and scope

LinkSys `TlsConfig.enabled` gates the local verification, CA, and client identity
block before reference resolution and runtime publication. A disabled block is
inert even when it retains missing or invalid certificate references. It must not
cause `Accepted=False`, `ResolvedRefs=False`, or a failed TLS readiness gate.
Re-enabling the block restores the existing validation and reference checks.

`resolved_ca_certificates` and `resolved_client_certificate` are Controller-owned
transport fields. Clear them before the disabled early return. LinkSys parsing
already clears the owner's previous Secret dependencies before rebuilding them,
so a disable update removes obsolete TLS dependency edges. Independent auth
references remain required. Credential-source providers also omit disabled TLS
from their manager config and credential-input revision list.

## Rationale and evidence

- `edgion-resources/src/resources/common/tls.rs` documents `enabled` as the
  on/off gate for local TLS policy, while secure endpoint schemes still select TLS.
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/auth_spec.rs`:
  `TlsSpec for TlsConfig::resolve` clears resolved fields, checks enablement, then
  runs the existing Secret/ReferenceGrant resolution pipeline.
- `handlers/link_sys.rs`: `parse` clears old dependency edges;
  `resolve_credential_source` collects only enabled TLS inputs.
- `handlers/san_guard.rs`: `check_link_sys_san` checks only enabled blocks across
  Redis, Etcd, Elasticsearch, Kafka, Webhook, HttpDns, and CredentialSource.
- `edgion-common/src/link_sys_core/credential.rs`: `tls_ready_inner` does not wait
  for inactive references. Gateway `credential_readiness` uses this result before
  provider mapping, in both full and partial updates.
- Redis mapping filters disabled local policy while retaining `rediss://` TLS;
  other providers keep their existing transport-selection and TLS-loading paths.
  HTTP providers use `build_outbound_client` and `apply_resolved_outbound_tls`;
  active TLS policy continues through `resolve_outbound_tls`, as required by
  [the unification rules](../../../00-design-rules.md#d-cross-cutting-unification--one-canonical-implementation-per-mechanism).
- `edgion-gateway/src/link_sys/providers/webhook/manager.rs`:
  `WebhookMutationGuard::prepare` removes inactive TLS before passing the connection
  to `ManagedOutboundClient`. That engine uses TLS block presence for Service target
  scheme and HTTP client construction; passing a disabled block would still select
  HTTPS or fail on unresolved references. URL targets retain their explicit scheme.

The original defect was before provider mapping: the Controller resolved unused
references, and Gateway readiness returned false for unresolved inactive refs.
Fresh clients were skipped; full and partial updates could preserve an old client
instead of publishing the requested configuration. SAN/hostname admission also
inspected inactive policy. Credential-source resolution errors prevented desired
source registration. These are different consequences of missing enablement gates.
Independent review additionally traced the Webhook manager's downstream consumer:
fixing only the front-end gates left it consuming inactive policy. Filtering at
the LinkSys-specific preparation boundary closes that path without changing shared
plugin-client contracts.

## Alternatives and limits

Do not remove readiness checks for enabled blocks or conflate TLS enablement with
authentication readiness. Do not disable URL-implied TLS to make a resource ready.
This correction does not redesign full/partial client replacement or change the
global `OutboundTlsConfig` (which has no enable switch). Webhook uses its own
manager publication path, and HttpDns has no generic runtime store; the ordinary
Redis/Etcd/Elasticsearch/Kafka store consequences do not apply identically to them.
Plugin-local TLS admission and resolution paths are outside this LinkSys fix.

The historical `g9a-007` resolution unified reqwest CA/mTLS loading. Its old symbols
`apply_insecure_skip_verify` / `apply_to_reqwest_builder` are historical, not the
current call chain, and did not establish disabled-block readiness behavior.

## Verification and re-evaluation

Fresh verification on 2026-09-11:

- `cargo test -p edgion-common --lib link_sys_core::credential::tests`: 34 passed,
  including all seven provider arms with disabled/enabled/unresolved references.
- `cargo test -p edgion-controller --lib conf_mgr::sync_runtime::resource_processor::handlers::link_sys::tests`:
  34 passed, including status/dependency replacement and credential-source TLS
  exclusion without bypassing required bootstrap authentication.
- `cargo test -p edgion-controller --lib san_guard`: 10 passed.
- `cargo test -p edgion-gateway --lib link_sys::providers::webhook::manager::tests`:
  23 passed. The new preparation matrix covers Service and HTTP/HTTPS URL targets,
  missing/invalid embedded material, and disable/enable/disable transitions.
  Enabled-material rejection is tested with `verify: true`; the existing
  `verify: false` material-parsing policy is unchanged.
- `cargo test -p edgion-gateway --lib rediss_with_disabled_tls_block_still_enables_tls`:
  1 passed, preserving secure-scheme TLS despite inactive custom material.
- `target/debug/test_client -g --dynamic-scenario LinkSysDisabledTls`: passed
  against freshly built binaries and the shared Controller/Gateway/Redis topology.
  Actual Redis DB 0/1 marker reads prove fresh creation, replacement, preservation
  under enabled unresolved TLS, and recovery after disabling. Resources and markers
  are cleaned up. No Kubernetes deployment or every-provider live transport sweep
  was run; other provider boundaries are covered by the targeted tests above.
- `cargo fmt --all` and `cargo fmt --all -- --check`,
  `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets`: passed. Clippy retains existing repository
  warnings; this verification is not a warning-free claim.
- Agent-doc consistency, SSA force, metrics inventory, and Pingora source-policy
  guards passed. Dynamic enumeration includes the new ID; Bash syntax and the four
  Redis-prerequisite selection checks passed.

Independent read-only review found the Webhook consumer omission and an overly
broad test-runner Redis prerequisite. Both were corrected and re-reviewed as LGTM.
Historical `g9a-007` evidence is separate from these fresh results.

Re-evaluate when adding a provider, changing TLS enablement semantics, changing
reference ownership, or altering full/partial publication gates. Provider TLS
dependencies and scheme behavior must be reviewed together with front-end gates.

## Resolution references

- `g9a-007`: historical correction unifying outbound reqwest TLS loading.
- `disabled-tls-publication-readiness`: fixed; inactive LinkSys TLS no longer
  blocks reference resolution/readiness or changes Webhook transport selection.
