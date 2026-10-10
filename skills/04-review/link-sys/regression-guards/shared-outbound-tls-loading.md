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
references remain required. Credential-source providers retain disabled TLS in the private actual-owner
snapshot but omit its material and credential-input revisions. The block remains
inert through the canonical owning selection.

## Rationale and evidence

- `edgion-resources/src/resources/common/tls.rs` documents `enabled` as the
  on/off gate for local TLS policy, while secure endpoint schemes still select TLS.
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/auth_spec.rs`:
  `resolve_tls` consumes the actual LinkSys TLS field tokens; disabled selections
  expose no active references. LinkSys `resolve_for_spec` clears old material
  before validation and applies only the complete resolved TLS block.
- `handlers/link_sys.rs`: `parse` clears old dependency edges;
  `resolve_credential_source` collects only enabled TLS inputs.
- `handlers/san_guard.rs`: `check_link_sys_san` checks only enabled blocks across
  Redis, Etcd, Elasticsearch, Kafka, Webhook, HttpDns, and CredentialSource.
- `edgion-common/src/link_sys_core/credential.rs`: `tls_ready_inner` does not wait
  for inactive references. Gateway `credential_readiness` uses this result before
  provider mapping, in both full and partial updates.
- Redis's owning PreparedRedisTls filters disabled local policy while retaining
  `rediss://` TLS and its existing empty-local global inheritance. It proves source
  and whole active pairing before replay, retains one captured snapshot across
  retries, then builds one connector through the shared rustls conversion core.
  Fred mapping receives that connector without resolving TLS again;
  other providers keep their existing transport-selection and TLS-loading paths.
  HTTP providers use `build_outbound_client` and `apply_resolved_outbound_tls`;
  active TLS policy continues through the canonical `outbound_tls` resolver, as required by
  [the unification rules](../../../00-design-rules.md#d-cross-cutting-unification--one-canonical-implementation-per-mechanism).
- `edgion-gateway/src/link_sys/providers/webhook/client.rs`:
  WebhookCandidate retains the actual owner and captured snapshot. Its owning
  ManagedOutboundClient entry checks source and active pairing, then selects
  Service target scheme and canonical HTTP construction from the proved active
  field. Inactive TLS uses native defaults without cloning/removing config or
  consuming inherited policy. URL targets retain their explicit scheme.
- Elasticsearch's EsCandidate retains the shared actual PreparedLinkSysTls
  container and checks active inherited authority before persistent planning.
  Its fixed-header owning HTTP factory derives config/auth from that owner and
  borrows the same captured TLS selection. Absent/disabled TLS retains native
  HTTPS trust without global inheritance. Bulk and recovery reuse the Client;
  init continues publishing unhealthy after a failed initial probe. Ordinary
  custom failure retains a callable prior publication; forced refresh withdraws
  it before replacement. Existing redirects, proxy use and system DNS remain
  unchanged; this slice establishes no egress/proxy verdict or auth-ref authority.
- KafkaCandidate uses PreparedLinkSysTls with a fixed actual Kafka variant and
  checks source, whole active pairing and consumed inherited authority before
  replay. Its fixed Common rustls entry uses the sole concrete conversion core.
  Absent/disabled local TLS yields no connector; enabled System TLS supplies an
  explicit connector because rskafka 0.6 uses None as plaintext. Kafka's private
  connection input retains the actual owner and constructed connector Arc across
  init, probe recovery and SDK broker reconnects. It does not clone TLS config or
  resolved Secrets or reread global defaults. Pending/Invalid SASL remains a
  construction error, without claiming auth-reference authority. Ordinary custom
  failure can retain prior callable publication; forced refresh withdraws it.
  Pure preparation/connector tests do not establish a live Kafka TLS handshake.


The detached `ManagedOutboundClient::from_parts`, its raw Webhook HTTP factory,
and Common's raw rustls constructor have been retired after all production
consumers migrated. Managed construction requires an actual bound owner and its
proved TLS selection. The obsolete global-loading resolver convenience wrapper
is also removed; snapshot, audited, actual-owner and effective conversion cores
remain canonical. Generic HTTP construction, preconfigured TLS, process bootstrap,
and `apply_resolved_outbound_tls` remain live supported boundaries. Unit tests use
actual Webhook/ForwardAuth/Redis owners or the existing private effective crypto
converter, preserving certificate parsing and policy failures without a raw API shim.

The original defect was before provider mapping: the Controller resolved unused
references, and Gateway readiness returned false for unresolved inactive refs.
Fresh clients were skipped; full and partial updates could preserve an old client
instead of publishing the requested configuration. SAN/hostname admission also
inspected inactive policy. Credential-source resolution errors prevented desired
source registration. These are different consequences of missing enablement gates.
Independent review additionally traced the former Webhook manager's downstream
consumer: fixing only the front-end gates left it consuming inactive policy.
Historical filtering at LinkSys preparation fixed that defect. The current owning
selection preserves that gate and proves actual source/material before replay;
the scoped plugin-client migrations through R11 and obsolete-entry retirement in
R12b are complete. Fresh live/admission/conformance acceptance remains open in the
reference task; these source results do not establish it.

### Controller OAuth2 owning transport

CredentialSource resolution clones the scrubbed actual LinkSys once, then moves
this generation's authorized TLS material into that private owner. Its Arc wrapper
has neither Serialize nor Debug; the returned conf_sync resource has no resolved
CredentialSource TLS payload. Registry clones share the Arc instead of cloning
resolved Secrets. Bootstrap bytes continue to be reread under existing fences.

Before awaiting DNS preparation, the manager captures PreparedLinkSysTls from
that actual owner and validates active inheritance. OAuth2Provider derives its
endpoint, scopes and egress policy from the same owner and builds through
build_outbound_client_with_tls_selection using the captured defaults. It keeps
only the Client and existing request metadata after construction. Absent/disabled
local TLS retains secure HTTPS with native trust and consumes no global policy.
Raw-owner UID/generation, frozen references, Grant revision, Secret UID/RV,
issuer/fallback checks, LeaderFence and publication/CAS remain independent and
unchanged: a stable captured owner is not authority to use stale credentials.
Unit construction tests do not establish a live issuer TLS handshake.

## Alternatives and limits

Do not remove readiness checks for enabled blocks or conflate TLS enablement with
authentication readiness. Do not disable URL-implied TLS to make a resource ready.
This correction does not redesign full/partial client replacement or change the
global `OutboundTlsConfig` (which has no enable switch). Webhook prepares a managed
client before publication in the unified typed runtime table, rather than using
the persistent-provider runner. HttpDns now owns a prepared immutable runtime in
that same table; it publishes the latest configuration's fallback when preparation
fails instead of adopting Webhook's ordinary last-good behavior. See
[prepared HTTPDNS generations](httpdns-prepared-runtime.md).
Plugin-local TLS admission and resolution paths are outside this LinkSys fix.

The historical `g9a-007` resolution unified reqwest CA/mTLS loading. Its old symbols
`apply_insecure_skip_verify` / `apply_to_reqwest_builder` are historical, not the
current call chain, and did not establish disabled-block readiness behavior.

## Verification and re-evaluation

Historical verification on 2026-09-11 (old manager test paths below predate the
provider-owned runtime refactor; current preparation tests live in client.rs):

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
- `redis-tls-integration-coverage`: the isolated `LinkSys/RedisTls` fixtures now
  exercise successful Redis handshakes through the complete Controller, conf_sync,
  Gateway, Fred, and Redis path. The suite covers scheme-only `rediss://` with the
  generated CA exposed through the documented native trust source, an explicit
  `caCertificateRefs` private CA, and mTLS with `clientCertificateRef`. Each client
  readiness check is bounded to 20 seconds. A fresh 2026-09-13 standard runner call,
  `run_integration.sh --no-prepare -r LinkSys -i RedisTls`, passed all 73 Redis
  LinkSys assertions, including the three new handshakes, plus resource convergence,
  unknown-field, requeue-loop, and cache-residue checks. Its first service probe
  exposed and drove correction of missing container-side TLS ports before the
  successful rerun. Container reuse across later work directories is a separate
  runner invariant: see
  [Redis TLS container reuse](../../testing/regression-guards/redis-tls-container-reuse-certificate-identity.md).
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
- `redis-tls-integration-coverage`: fixed; successful default-trust, custom-CA,
  and mutual-TLS Redis handshakes have isolated integration coverage.

## Etcd owning and trusted-bootstrap inputs

Etcd now prepares actual LinkSys source and whole active TLS pairing before
persistent reuse. Cold construction uses the captured defaults and fixed
provider capability; absent/disabled TLS does not inherit global policy, including
HTTPS endpoints. Prepared ordinary and election SDK options are retained in one
immutable connection input across every connection path. Unsupported TLS policy
fails before any dial. The trusted Controller bootstrap input uses direct disk
PEM and YAML credentials through the same private borrowed TLS core; it creates
no Secret or resource reference. Disk CA masks whole inherited validation;
declared global client identity wins and never falls back to disk on failure.
This preserves upstream SDK semantics rather than establishing new root-store,
PEM-cloning or end-to-end zeroization guarantees.

## OTLP HTTPS inheritance at actual-owner preparation

PreparedOtlpTls preserves OTLP's explicit metadata/config validation and proves
actual source before replay. Active local pairing and consumed inherited proofs
precede OtlpCandidate fingerprints and persistent reuse. Unlike Etcd's explicit
local enablement gate, OTLP HTTPS with absent/disabled local TLS inherits the
captured globals via empty local policy. HTTP consumes no global TLS and rejects
an enabled local block. The closed adapter feeds the canonical audited resolver;
Gateway performs the one native tonic014 SDK conversion and retains its Endpoint
and actual owning input through retries/reconnects. Authentication authority and
other providers' distinct native TLS policies are not changed by this adapter.
