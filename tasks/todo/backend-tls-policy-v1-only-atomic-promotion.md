# Promote BackendTLSPolicy handling atomically to v1 only

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | pending |
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
