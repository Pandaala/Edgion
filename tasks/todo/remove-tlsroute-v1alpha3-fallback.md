# Remove the TLSRoute v1alpha3 API Fallback

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-10 |
| Status | pending |
| Type | refactor |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Remove the remaining TLSRoute API-version fallback atomically so every controller input, watch, persistence and status path uses canonical `gateway.networking.k8s.io/v1`. This is existing implementation debt under the atomic API-upgrade rule, not a claim that the migration is already complete. It is unrelated to the Gateway's Stage-1/Stage-2 stream-plugin fallback.

## Evidence

Verified statically at `78a25c3a`; no migration test was run when recording this task.

- `edgion-controller/src/conf_mgr/conf_center/kubernetes/version_detection.rs:127`: `probe_tls_route_version` probes v1, then v1alpha3.
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/controller.rs:771`: the detected version selects a `TLSRoute` or `TLSRouteAlpha3` watcher.
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/storage.rs:109`: CRUD API selection follows that detected version.
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/tls_route.rs:316`: `TlsRouteAlpha3Handler` retains compatibility processing and status conversion.
- `edgion-controller/src/conf_mgr/conf_center/common.rs:161`: `parse_resource_content` converts v1alpha3 to v1; existing FileSystem tests explicitly accept this conversion.
- `edgion-resources/src/resources/tls_route.rs:103`: the legacy resource type and bidirectional conversions remain.

## Requirements

- Apply the atomic-only API-version upgrade rule in `skills/00-design-rules.md` section L and `skills/08-kubernetes/04-api-version-upgrade.md`.
- Use v1 consistently for preflight, watches, CRUD, processors, status and internal registration. Remove legacy probes, watchers, handlers, conversions and compatibility shims together; do not introduce a new fallback.
- Audit raw-input boundaries including FileSystem, etcd and Admin paths. Validate `apiVersion` before typed deserialization and insertion into raw storage or the workqueue; reject missing, malformed and non-canonical versions.
- Preserve one workqueue and one worker per resource kind, serial processing, and existing requeue/status ordering.
- Keep fixtures, examples and version sources coherent. Do not alter vendored CRDs merely to hide versions served by upstream.

## Scope

### In scope

- TLSRoute API-version selection, ingress validation, resource wiring and directly related unit tests/documentation needed for one coherent migration.

### Out of scope

- TLSRoute routing semantics, Stage-1/Stage-2 plugin policy, TLS handshakes, BackendTLSPolicy behavior and Gateway rustls support.
- Editing existing integration-test files without separate exact-path authorization; record integration coverage needs independently.
- Implementing the migration as part of adding this task record.

## Acceptance criteria

- No active TLSRoute v1alpha3 probe, watcher, CRUD/status branch, handler, conversion or registry shim remains.
- Tests exercise actual raw-input entry points: v1 succeeds, while missing/malformed/legacy versions fail before publication or enqueue. Generated-type deserialization alone is not sufficient evidence.
- Canonical v1 watch, persistence and status behavior remains covered, including the single-worker invariant.
- Targeted unit tests, repository checks and documentation-link validation pass; any required integration work follows its separately authorized scope.
