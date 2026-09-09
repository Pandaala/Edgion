# Unify Canonical Route Parent Status Identity

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Define and apply one canonical route-parent status identity from Controller writers through storage and comparison to Gateway config publication, so defaulted forms, foreign-controller entries, and distinct Gateway/delegation parents cannot collide or override one another.

## Requirements

- Inventory every Controller producer and consumer of `RouteParentStatus` across HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute, and EdgionTls, plus every Gateway config-publication consumer that selects `Accepted` parent status, before changing behavior.
- Define one reusable canonical identity that normalizes absent `group` to `gateway.networking.k8s.io` and absent `kind` to `Gateway` where Gateway API ParentReference defaults apply.
- Include normalized group and kind plus applicable namespace, name, sectionName, port, and controllerName in Controller update, retention, deduplication, semantic comparison, and Gateway acceptance decisions.
- Make Gateway config publication consider only the `RouteParentStatus` entry owned by the active configured controller identity. Obtain that identity from the shared/configured source used by the Controller/Gateway configuration path; the Controller name is customizable and must never be hardcoded to the default literal.
- Ensure a foreign controller's `Accepted=True` entry cannot override or precede the active Edgion controller's `Accepted=False` entry for the same canonical parent.
- Preserve distinctions for route-delegation reverse locks and any non-Gateway parent kinds; same namespace/name coordinates must not merge different group/kind identities.
- Update the complete writer/storage/comparison/Gateway-consumer chain atomically. Do not land an opportunistic partial fix in only one route family, one lifecycle phase, or only the Controller or Gateway side.
- Preserve the canonical one-workqueue/one-worker model, event enqueueing, serial handler processing, status-write ownership, requeue behavior, and delete/create coalescing invariants.
- Reuse a shared identity helper or type; do not introduce route-family copies with subtly different defaulting or field sets.
- Do not add logs or metrics. Any existing Controller diagnostic must remain bounded and redact resource or credential detail according to the logging rules.
- Add deterministic focused unit tests for omitted/explicit defaults, group/kind collisions, namespace fallback, section and port distinctions, customizable controllerName ownership, foreign-controller precedence, stale retention, deduplication, status semantic equality, and Gateway publication filtering.
- Keep every existing `edgion-tests/**` file frozen unless their exact paths are explicitly authorized; record separately any live scenario that cannot be added without modifying a frozen file.

## Scope

### In scope

- Controller route-parent status identity construction, lookup, update, retention, deduplication, and semantic comparison.
- Gateway config-publication filtering of `RouteParentStatus` by canonical parent identity and the active configured controller identity.
- A shared canonical identity representation and focused in-crate tests.
- All route/resource families that persist `RouteParentStatus`.

### Out of scope

- Per-request Gateway route matching after configuration publication.
- New Gateway API fields, delegation behavior, workqueues, status writers, logs, or metrics.
- Existing integration-test modifications unless their exact paths are explicitly authorized.

## Acceptance criteria

- Every audited Controller and Gateway config-publication status path uses the same canonical identity and field normalization from writer through consumer.
- Omitted and explicit Gateway defaults identify the same parent, while different groups or kinds at identical coordinates remain distinct.
- Namespace/name, applicable sectionName/port, and controllerName differences are retained exactly where they define separate status ownership.
- Gateway publication selects status for the active configured controller name without hardcoding the default, and ignores a foreign controller's conflicting `Accepted` condition regardless of list order.
- Update and retention cannot overwrite, duplicate, or preserve a stale status entry because another parent shares only partial coordinates.
- Semantic comparison ignores representational default differences but detects every meaningful identity or condition change.
- Focused tests cover all route/resource families and the listed identity boundaries without changing existing integration files.
- Workspace format/check/Clippy, documentation validation, and repository static guards pass.
