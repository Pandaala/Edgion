# Complete RequestMirror Standard Validation and Service Status

## Meta

- Status: pending
- Priority: P1
- Category: correctness
- Severity: medium
- Discovered in: B0304 at `bf5e3a6e`

## Problem

RequestMirror standard fields are not validated consistently outside Kubernetes admission. `RequestMirrorPluginConfig::validation_error()` validates only Edgion tuning limits, while preserved EdgionPlugins configuration and non-Kubernetes HTTPRoute inputs can reach runtime with both `fraction` and `percent`, invalid sampling ranges, an empty Service name, or a missing/invalid port. Runtime sampling then silently prioritizes or normalizes invalid values instead of rejecting the configuration.

The same boundary has an incomplete reference lifecycle. A missing mirror Service is not reported as `ResolvedRefs=False/BackendNotFound`, and mirror Service dependencies are not comprehensively registered, replaced, removed and requeued for both HTTPRoute and EdgionPlugins publishers. The vendored Gateway API contract requires an unresolved referent to remain unconfigured.

## Required outcome

- Allow neither sampling field, which means 100% mirroring; reject simultaneous `fraction` and `percent`; validate whichever field is present.
- Require denominator `>= 1`, numerator within `0..=denominator`, and percent within `0..=100`.
- Require a core Service reference with a non-empty name and a required valid port.
- Use shared validation so Controller and Gateway diagnostics remain equivalent for EdgionPlugins and non-Kubernetes HTTPRoute input.
- Preserve lenient admission while mapping invalid HTTPRoute rules to the existing invalid-rule/`PartiallyInvalid` behavior and invalid EdgionPlugins to `Accepted=False`.
- Report a missing Service as `ResolvedRefs=False` with `BackendNotFound`.
- Register mirror Service dependencies for both publishers, fully replace them on apply, remove them on delete, and requeue dependents when Service availability changes.
- Preserve existing ReferenceGrant enforcement, immutable publication and best-effort isolation of the primary request.

## Scope

Start with:

- `edgion-resources/src/resources/http_route.rs`
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/request_mirror.rs`
- HTTPRoute preparse/status and Service dependency indexing
- EdgionPlugins Controller/Gateway validation parity and Service dependency indexing
- focused resource and Controller tests

Do not turn this into a Gateway RequestMirror runtime refactor. Reuse the existing validation, status, ReferenceGrant and workqueue mechanisms; do not create a parallel predicate or dependency system.

## Verification

Add deterministic matrices for sampling boundaries, Service identity/port validation, Controller/Gateway diagnostic parity, missing-Service status, replacement/delete cleanup and Service-triggered requeue for both publishers. Run focused resource/Controller tests, CRD coverage where applicable, and the normal repository gates. Existing integration files remain frozen; add a separate test-gap task only if deterministic local coverage cannot prove a required cross-process contract.
