# Promote BackendTLSPolicy handling atomically to v1 only

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | in-progress |
| Type | refactor |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Remove Edgion's BackendTLSPolicy v1alpha3 compatibility path and make `gateway.networking.k8s.io/v1` the only Controller, FileSystem/etcd, resource, status, fixture, and documentation contract. The repository's mandatory API-version policy requires atomic promotion and forbids fallback probing, dual handlers, and legacy conversion shims, but the current implementation still carries all three.

## Requirements

- Watch and preflight BackendTLSPolicy through the canonical v1 type only.
- Delete v1alpha3 fallback detection, watcher registration, resource conversion structs, handler branches, and compatibility tests.
- Reject non-v1 FileSystem and etcd BackendTLSPolicy inputs before they enter the raw store or workqueue.
- Keep status reads and writes on the same v1 type used by the watcher.
- Update current architecture, feature, configuration, and user documentation to describe v1 only.
- Preserve the committed upstream Gateway API bundle as an upstream artifact; an additional version served by that bundle must not create an Edgion fallback watcher.
- Update dependent cross-resource lookups so they use the single v1 processor registry.

## Scope

### In scope

- Controller Gateway API version detection, storage, resource registration, BackendTLSPolicy handlers, and status paths.
- `edgion-resources` BackendTLSPolicy legacy types and conversions.
- FileSystem/etcd input validation, fixtures, tests, skills, and current docs.

### Out of scope

- Hand-editing generated upstream CRD schemas solely to hide a version the pinned upstream bundle still serves.
- TLSRoute version promotion unless required by a shared version-detection abstraction; otherwise track it separately.
- Gateway TLS runtime behavior or Gateway rustls support.

## Acceptance criteria

- Repository searches find no live `BackendTLSPolicyAlpha3`, v1alpha3 watcher, fallback probe, conversion, compatibility fixture, or current-doc promise.
- The Controller registers exactly one BackendTLSPolicy worker and watcher using v1.
- Preflight, watch, raw-store identity, CRUD, and status updates all use v1.
- FileSystem and etcd reject a BackendTLSPolicy whose `apiVersion` is not `gateway.networking.k8s.io/v1` before enqueue.
- Cross-resource BackendTLSPolicy lookups retain their existing conflict and requeue behavior through the v1 registry.
- Relevant unit, integration, and Gateway API conformance tests pass.
- Workspace check, Clippy, CRD coverage, agent-doc validation, and repository guards pass.

## Related guidance

- `skills/00-design-rules.md`
- `skills/08-kubernetes/04-api-version-upgrade.md`
- `skills/01-architecture/02-gateway/07-backends.md`
- `skills/02-features/03-resources/12-backend-tls-policy.md`
- `skills/05-testing/SKILL.md`

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-31 | Defer the promotion from the Gateway file audit. | The current compatibility path spans resource types, Controller startup and watchers, storage, handlers, status, fixtures, and documentation; removing only one branch would leave a mixed-version path. |
| 2026-09-01 | Remove the v1alpha3 path atomically and keep only the canonical v1 watch, storage, status, and static-input contract. | The resource type, Controller watcher and handler, CRUD path, FileSystem/etcd parser, cross-resource registry lookup, fixtures, and current documentation now share one v1 identity; the pinned upstream CRD bundles remain unchanged. |

## Implementation evidence

- Removed the v1alpha3 resource type, conversions, version probe, watcher and handler branches, CRUD fallback, and dual-type registry downcast.
- FileSystem and etcd share a parser that rejects a missing or non-v1 BackendTLSPolicy `apiVersion` before raw-store insertion; Kubernetes CRUD rejects non-v1 payloads instead of projecting them.
- Updated current fixtures and documentation to v1 and retained the upstream Gateway API CRD bundles without modification.
- Focused resource, Controller handler, static-input, Kubernetes storage, and version-detection tests pass. Workspace check, Clippy, formatting, CRD coverage, agent-doc validation, and repository guards pass.
- Cluster-backed integration and Gateway API conformance verification remains before the task can be marked completed.

### Independent review and correction

- Initial review found that Admin create/update/validate could deserialize submitted v1alpha3
  TypeMeta into the canonical type before the Kubernetes storage check.
- The shared Admin parser now invokes the same raw API-version validator used by FileSystem/etcd;
  Kubernetes DynamicObject storage invokes the same canonical primitive.
- Added direct regressions for update-version enabled/disabled parsing, Admin create, Admin update
  with preservation of the current v1 object, and the read-only validate endpoint.
- Re-review closed the Must Fix and found no remaining Must Fix.

### Verification record

- `cargo test -p edgion-controller backend_tls_policy --lib`: 30 passed.
- `cargo test -p edgion-resources backend_tls_policy --lib`: 12 passed.
- `cargo test -p edgion-controller --lib api::common::tests`: 8 passed.
- `cargo test -p edgion-controller --lib api::namespaced_handlers::update_identity_tests`: 9 passed.
- `cargo test -p edgion-controller --lib api::validate_handlers::tests`: 9 passed.
- `cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets`, formatting,
  CRD coverage, agent-doc, SSA, metrics, Pingora-source, and `git diff --check`: passed.
