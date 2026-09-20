---
name: backend-tls-policy-canonical-api-ingress
description: Use when reviewing BackendTLSPolicy API versions, raw document ingress, Kubernetes watch identity, or status writes.
knowledge_type: regression-guards
---

# BackendTLSPolicy uses one canonical API identity

## Conclusion and scope

The Controller accepts BackendTLSPolicy as `gateway.networking.k8s.io/v1`.
Kubernetes preflight, watch, CRUD, processor lookup, and status writes use that
identity. FileSystem, etcd, and Admin raw-document ingress reject a missing or
non-v1 `apiVersion` before typed deserialization can discard the submitted
TypeMeta. This is an ingress contract, not a promise that directly deserializing
the generated Rust resource validates its submitted API version.

`BackendTLSPolicySpec.target_refs` identifies the policy's backend targets;
`validation` describes their server-certificate authentication. The API promotion
does not change those fields, arbitration, requeue semantics, or Gateway TLS.

## Rationale and evidence

The authority is `skills/00-design-rules.md` section L and
`skills/08-kubernetes/04-api-version-upgrade.md`: one atomic API upgrade, without
fallback probes, dual watchers, or legacy conversion shims.

- `edgion-resources/src/resources/backend_tls_policy.rs`: the CustomResource
  declaration specifies group `gateway.networking.k8s.io` and version `v1`.
- Controller `conf_mgr/conf_center/kubernetes/version_detection.rs` records v1
  in `GATEWAY_API_RESOURCES`; `controller.rs` registers one typed watcher.
- `kubernetes/storage.rs` constructs the v1 API resource and validates raw CRUD
  payload versions; `kubernetes/resource_controller.rs` derives the status-patch
  API from the same resource type's group/version/kind.
- `conf_center/common.rs::parse_resource_content` checks the raw document before
  deserialization. FileSystem and etcd invoke it before raw-store insertion.
- `api/common.rs::parse_resource_and_update_version` invokes the same validator
  before parsing or assigning an internal resource version. Admin create,
  update, and validate therefore cannot normalize old TypeMeta into accepted v1.

The earlier fix removed `BackendTLSPolicyAlpha3`, conversions, fallback probing,
dual handler registration, and legacy registry downcasts. A subsequent review
caught the Admin ingress gap and moved its validation ahead of typed parsing.

## Alternatives and limits

Additional versions served by the pinned upstream CRD remain an API-server
concern. They do not authorize fallback Edgion watchers. Do not edit upstream
bundles just to hide their served versions. TLSRoute promotion and Gateway
rustls support remain outside this correction.

Static-input and Admin rejection tests do not establish Kubernetes admission or
real TLS connectivity. Upstream conformance reports from filtered runs can list
unexecuted tests as passed; report only the subtests actually executed.

## Verification and re-evaluation

Verification resumed on 2026-09-09; the earlier 2026-09-01 results are historical.
The follow-up adds an Admin parser negative matrix for absent, null, numeric,
empty, and unsupported versions across JSON/YAML and both internal-version
update modes. Runtime resource fields and behavior are unchanged, so user-facing
configuration documentation needs no change.

- Controller BackendTLSPolicy unit tests: 30 passed; resource tests: 12 passed;
  Controller Admin API tests: 149 passed; Kubernetes version detection: 4 passed.
- CRD coverage: 14 coverage and 8 plugin-discriminator tests passed. The fixture
  scan found 45 local/Kubernetes BackendTLSPolicy documents, all using v1.
- On OrbStack Kubernetes v1.33.9+orb1, `Status/BackendTLSPolicy` reported 7 passing
  cases: 5 exercised assertions, while 2 returned early because Kubernetes
  admission already rejects their malformed validation combinations.
- Kubernetes batch suites passed: `Gateway/TLS/BackendTLS` (6),
  `Gateway/TLS/BackendTLSGatewayCert` (2), `Gateway/TLS/BackendTLSSAN` (2), and
  `HTTPRoute/Backend/BackendTLSSNIHostOverride` (3). The BackendTLS single-suite
  entry point also passed all 6 cases. Both entry points ran without manually
  supplied endpoint IPs after the [runner correction](../../tls/regression-guards/backend-tls-pool-isolation.md).
- The Kubernetes `BtlspRefFlap` dynamic scenario passed, exercising invalid
  references, recovery, and repeated policy/reference changes.
- Pinned upstream conformance v1.5.1 passed all 6 actually executed
  `TestConformance/BackendTLSPolicy` tests: basic TLS and ConfigMap rotation,
  conflict resolution, invalid CA reference, invalid kind, observed-generation
  bump, and SAN validation. The filtered `GATEWAY-HTTP` run used loopback TAL
  port forwarding; it is not a full-profile conformance claim. Four local Go
  profile-selection unit tests also passed.
- Formatting, workspace all-target check and Clippy passed; Clippy retains
  existing warnings. Agent-doc, SSA-force, metrics-inventory, Pingora-source,
  and shell-selection guards passed. No full-workspace test run was performed.

The live images (`btlsp-v1-verify_arm64`) were built from the 23b35905 source
baseline. An external change advanced HEAD to 6a5e428a during verification;
Controller/resources/common, Gateway backend/TLS/route code, and the selected
TLS test paths were unchanged between those revisions. Workspace checks were
rerun at the newer HEAD using an isolated target directory after an initial
shared-target build encountered disappearing dependency metadata.

The original full CRD installation failed on an unrelated EdgionTls structural
schema error. These live tests used an ephemeral CRD-root copy removing only its
invalid `allowedSans.items.additionalProperties: false` declaration; the entire
upstream Gateway API bundle remained byte-identical. The installed BackendTLSPolicy
CRD served v1 as storage and v1alpha3 as non-storage, while Edgion used only v1.
That made this historical BackendTLSPolicy verification conditional rather than
evidence that the original complete CRD set installed. The EdgionTls defect was
subsequently corrected and guarded by
[the declared-object structural-schema rule](../../resources/regression-guards/declared-object-crd-structural-schema.md).

The shared integration topology was reused for conformance with the `edgion`
GatewayClass, all-namespace watches, and the same mTLS conf-sync configuration.
Initial setup attempts exhausted OrbStack's Service IP range and selected stale
Gateway startup listeners. Removing completed fixture Services and restarting
the Gateway after conformance resources were ready resolved those test-environment
failures. Set `GATEWAY_PROXY_HOST=127.0.0.1` explicitly with `--port-forward` to
avoid OrbStack auto-detection selecting a nonlocal bind address. The final run
completed in 14.192 seconds; its executed-subtest log and report were retained
as `/tmp/edgion-btlsp-conformance.log` and
`/tmp/edgion-btlsp-conformance-report.yaml` for this session.

Re-evaluate on API-version upgrades, new raw-document ingress, Admin parser
changes, or changes to watcher/status type registration. Preserve negative
controls for missing/malformed versions and both JSON/YAML Admin inputs.

## Resolution references

- `backend-tls-policy-v1-only-atomic-promotion`: implementation already present;
  regression and conditional cluster verification completed on 2026-09-09.
  No new runtime behavior is
  introduced by the verification follow-up.
