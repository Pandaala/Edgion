# Enforce EdgionAcme HTTP-01 GatewayRef Ownership

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Make an HTTP-01 challenge serve only on the Gateway/listener selected by `EdgionAcme.spec.challenge.gatewayRef`. The raw reference already serializes through conf-sync, but the Controller does not validate or resolve it into a serving attachment, and the Gateway's global challenge store plus pre-route interception do not enforce target ownership. This is a design-first Controller/resources/Gateway change that must use Gateway API reference and listener semantics without requiring an HTTPRoute.

## Requirements

- Validate and default the reference's group, kind, namespace, name, optional `sectionName`, and optional `port` using Gateway API-style semantics. Reject unsupported targets and non-HTTP listener selections through normal resource status.
- When repository cross-namespace ReferenceGrant enforcement is enabled, a cross-namespace `edgion.io/EdgionAcme` reference to `gateway.networking.k8s.io/Gateway` requires a matching ReferenceGrant in the Gateway namespace. Register and clear the canonical `cross_ns_ref_mgr` dependency so grant addition, change, or removal requeues the EdgionAcme. Do not substitute the Route-to-Gateway `listener.allowedRoutes` exception without a separate explicit extension design.
- Register the selected Gateway as a dependency so Gateway creation, deletion, address changes, listener name/port/protocol changes, and whichever status/programming eligibility the design adopts requeue the EdgionAcme resource.
- Resolve a deduplicated set of compatible HTTP serving attachment identities: `sectionName` narrows by listener name, `port` narrows by listener port, both constraints must match when present, and neither selects every compatible HTTP listener on the referenced Gateway. A target resolving only to non-HTTP listeners publishes no attachment and fails closed.
- Publish the resolved non-secret attachment set to the Gateway. Keep raw operator input and resolved runtime identity distinct and apply `#[schemars(skip)]` to non-operator fields if a resource field is added.
- Index challenge snapshots by the resolved attachment set and enforce membership at pre-route HTTP-01 interception against the receiving Gateway/listener. Challenge serving must not require an HTTPRoute match.
- Preserve domain/Host normalization, token/key-authorization matching, exclusive expiry, full/partial/delete atomic publication, hot reload, and empty-host rejection.
- Preserve sensitive-data rules: never log token, key authorization, Secret material, or full resource specs. Do not add request-path tracing or high-cardinality metric labels.
- Do not conflate this attachment with `autoEdgionTls` parent references or change ACME account, issuance, DNS-01, certificate, or TLS-termination behavior.

## Scope

### In scope

- Controller EdgionAcme validation, status, dependency registration, and requeue behavior.
- The resolved EdgionAcme runtime representation in `edgion-resources` only if required by the reviewed design, including schema/conf-sync/redaction invariants.
- Gateway ACME challenge store and conf handler.
- Pre-route HTTP-01 interception and the canonical listener/Gateway identity or ParentReference matching helper needed to identify the receiving attachment.
- Canonical ACME architecture, feature, and EN/ZH operator documentation.
- Source-local controller, resource, store, and request-filter tests.

### Out of scope

- Requiring an HTTPRoute for HTTP-01 service.
- ACME account registration, order lifecycle, DNS-01 propagation, certificate storage, or downstream TLS behavior.
- `autoEdgionTls` parent-reference behavior.
- Editing existing `edgion-tests/**` files in the Gateway quality campaign.
- Adding a new metric or label unless the existing bounded status/access-log signals cannot express rejection.

## Acceptance criteria

- Missing, malformed, unsupported-kind, nonexistent, cross-namespace-denied, and non-HTTP listener targets receive deterministic fail-closed status without publishing a serving attachment.
- Namespace defaulting, `sectionName`, and `port` selection match established Gateway API behavior: both constraints must agree when present; port-only selects all compatible listeners on that port; neither selects all compatible HTTP listeners. Every attachment-affecting Gateway or grant change requeues.
- Every resolved attachment serves the challenge while a non-member co-located Gateway/listener with the same Host and token falls through normally.
- Hot retargeting an attachment set from A to B atomically stops removed members and enables added members without an unowned serving window; challenge update, expiry, and removal remain atomic.
- Pre-route interception compares the current receiving attachment and does not require route selection.
- Host, token, key authorization, expiry, observability, redaction, and metric-label contracts remain unchanged.
- Deterministic source-local tests cover validation, dependency requeue, target publication, selected/non-selected interception, hot retarget, and removal.
- The separate multi-Gateway integration-coverage task remains blocked until the serving attachment/API design is approved.
- Formatting, workspace check, Clippy, schema/CRD checks when applicable, documentation validation, SSA, metrics, Gateway no-tracing, and relevant repository guards pass.

## Document index

| Document | Status | Notes |
|----------|--------|-------|
| `enforce-edgion-acme-http01-gatewayref-ownership.md` | done | Analysis boundary and acceptance contract |
| `00-context.md` | not created | Create before implementation |
| `01-design.md` | not created | Required before implementation |
| `02-implementation.md` | not created | Create after independent design approval |
| `03-subtasks.md` | not created | Required with the approved multi-module design |

## Affected modules

| Module | Impact |
|--------|--------|
| Controller EdgionAcme handler | Validate, resolve, status, and requeue the target |
| EdgionAcme runtime representation | Carry a non-secret resolved serving attachment set if needed |
| Gateway ACME store/interception | Index and enforce listener/Gateway attachment membership |
| ACME documentation | State the executable gatewayRef contract |

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-31 | Defer from B0060 as a design-first end-to-end bugfix. | A store-local predicate cannot establish Controller validation, dependency requeue, resolved publication, or receiving-listener identity. |
| 2026-08-31 | Keep integration coverage in a separate blocked task. | The current suite has no HTTP-01 Gateway data-plane scenario, and its fixtures must follow the approved serving-attachment API rather than guess it. |
