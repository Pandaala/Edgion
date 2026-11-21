---
name: mirror-reference-grant-enforcement
description: Use when reviewing mirror backend authorization, delegation namespaces, and ReferenceGrant reload behavior.
knowledge_type: regression-guards
finding_id: request-mirror-reference-grant-enforcement
---

# Mirror backend ReferenceGrant enforcement

## Conclusion and scope

RequestMirror uses the existing `RefDenied` transport representation on
`BackendObjectReference.ref_denied`. The Controller replaces inbound or stale
markers on every reconciliation. The Gateway skips a denied mirror before
endpoint selection, task admission, request-body preparation, or writer creation,
records `mirror ref denied` through PluginLog, and continues the primary request.
This best-effort disposition was explicitly approved for this task; rejection of
the primary request is not required to prevent unauthorized mirror delivery.

Native HTTPRoute rule and backend filters authorize as
`gateway.networking.k8s.io/HTTPRoute` in the declaring route's namespace.
EdgionPlugins request-stage mirrors authorize as `edgion.io/EdgionPlugins` in the
plugin resource's namespace. Their grants are not interchangeable. Same-namespace
references require no grant; the existing Controller-global validation switch
continues to control cross-namespace authorization.

Mirror selection supports only core Service targets (`group: ""`, default
`kind: Service`). Unsupported groups/kinds fail shared standard validation and
are not published for selection, so granting a different resource cannot authorize a
same-named Service. This does not expand the mirror's backend capabilities.

## Rationale and evidence

The repository's vendored Gateway API v1.6.2 HTTPRoute schema requires a denied
cross-namespace mirror to report `ResolvedRefs=False/RefNotPermitted` and not be
configured for delivery. The canonical authority is
`config/crd/gateway-api/gateway-api-standard-v1.6.2.yaml`, the `requestMirror.backendRef`
description. The existing named-jump fix did not cover mirror references.

- `edgion-resources/src/resources/http_route.rs`: the mirror reference keeps
  `ref_denied` serialized for conf_sync and excludes it with `schemars(skip)`.
  It is internal metadata, not an operator-facing CRD extension.
- Controller `handlers/request_mirror.rs`: shared marking, filter traversal,
  exact declaring-resource identity, and standard reference errors, using the
  existing ReferenceGrant store rather than a second authorization implementation.
- `handlers/http_route.rs`: each route marks its own rule/backend mirrors.
  Rule/backend mirrors contribute to status and cross-namespace refs.
- Delegation (cascading match): every level is its own published route, so a
  parent's mandatory mirror filter is authorized as the parent's and a child's
  mirror as the child's, each under its own owner and dependency set. Nothing is
  inherited by copying. (History: the removed Controller flattening authorized child fields before cloning them into the parent; that path no longer exists.)
- `handlers/edgion_plugins.rs`: request-stage mirror markers, dependencies and
  `ResolvedRefs` status participate in existing parse/delete lifecycle. Disabled
  entries do not contribute reference errors.
- Gateway `plugins/http/request_mirror/plugin.rs`: the early refusal returns
  `GoodNext` without touching the request session. The existing mirror writer,
  body ownership, primary backend selection and named-jump enforcement are retained.

ReferenceGrant changes use the existing cross-namespace manager and serial workqueue.
Dependency sets are rebuilt on apply and cleared on delete; they are not a separate
queue or a data-plane grant store.

Mirror dependencies are registered before reading the grant store. HTTPRoute
clears the previous set once at parse entry and only extends it afterwards;
each route registers its own owner's dependency while authorizing its own
identity. Thus a concurrent grant revocation either precedes
the authorization read or finds the registered owner for requeue. The later
`reference-grant-dependency-registration-race` correction moved ordinary HTTPRoute
and GRPCRoute backend authorization onto the same shared register-before-read
ordering; see the
[general handler guard](../../handler-requeue/regression-guards/reference-grant-dependency-before-authorization.md).

Gateway `plugin_store.rs::can_retain_previous` rejects whole-runtime LKG when
either previous or incoming configuration has an executable request-stage
cross-namespace mirror. Invalid DSL/WAF updates then publish the existing sealed
HTTP 500 runtime in both `apply_delta` and `replace_all`. Checking only incoming
denial would miss updates that remove or retarget the mirror. Valid configurations
with denied mirrors still skip only the mirror. Disabled, policy-replaced, and
same-namespace mirrors do not change ordinary LKG behavior; in-flight Arcs remain
valid. The user explicitly approved this availability tradeoff to prevent stale
authorization, including mixed invalid-plugin updates.

## Alternatives and limits

Checking only Controller status leaves the data-plane bypass open. Checking only
the Gateway without transported denial information cannot distinguish authorized
targets. Filtering out the mirror before recording the denial also loses the
standard status and request diagnostic. Reusing root-route identity for inherited
mirrors permits the wrong namespace to authorize a child's traffic destination.

Grant revocation applies to new requests after the updated configuration reaches
the Gateway. Already-started mirror tasks retain their request snapshot and are
not cancelled retroactively. This task does not add an immediate revocation fence,
mirror BackendTLSPolicy support, or new load-balancing policy behavior.
Standard field validation and missing-Service status are separate from this
authorization rule and are covered by
[standard validation and Service dependency lifecycle](mirror-standard-validation-and-service-status.md).

## Verification and re-evaluation

Verified on 2026-09-09:

- `cargo test -p edgion-controller --lib mirror_`: 5 passed, including raw child
  and intermediate delegation, exact grant identity/name matching, marker
  replacement, dependency removal, compiled sibling copies, and disabled entries.
- `cargo test -p edgion-gateway --lib plugins::http::request_mirror::`: 26 passed,
  including a no-session/no-writer denial and wrong-kind refusal test.
- After the LKG correction, `cargo test -p edgion-gateway --lib mirror_` passed
  28 tests, and `plugins::http::plugin_store::compile_plugins_tests::` passed
  22 tests, including DSL/WAF delta/full replacement and ordinary LKG retention.
- `cargo test -p edgion-resources --lib mirror_denial_is_transport_metadata_not_operator_schema`:
  1 passed; the denial survives serde transport and is absent from schemars.
- `./cicd/checks/check_crd_coverage.sh` and the `plugin_crd_type_coverage` resource
  test passed.
- Workspace check/clippy, formatting, agent-doc validation, SSA force, metrics
  inventory, Pingora source policy and Gateway no-tracing checks passed. Clippy
  reports existing warnings; no full workspace test run was requested or performed.
- Deeper independent Controller and Gateway reviews found the LKG and
  dependency-ordering gaps missed by the initial review. Both reviewers returned
  LGTM after the corrections, and the corresponding unit tests were rerun.

- The canonical local RequestMirror suite and `RequestMirrorReferenceGrant`
  dynamic scenario passed using the same topology on 2026-09-09. The subsequent
  `cache_residue --mode fail` completed successfully (`wipe-all: total=39 errors=0`).
  Run evidence: `tmp/integration_testing/testing_20260909_154624/report.log`,
  `/tmp/edgion-mirror-dynamic.log`, and `/tmp/edgion-mirror-residue.log` (ephemeral).
  Kubernetes inventory wiring and live end-to-end verification were completed on
  2026-09-10. The Kubernetes `RequestMirrorReferenceGrant` scenario passed with
  selector-backed Services and runner-owned Deployments, covering the same grant,
  revocation, delivery, and non-delivery phases in cluster topology.

The local `RequestMirrorReferenceGrant` dynamic scenario uses the shared topology
and checks no grant, HTTPRoute-only authorization, authorization of both publishers,
and revocation. It combines Controller status, Gateway payload convergence, real
POST-body receipts, and the terminal request PluginLog denial. Positive controls
prove both backend listeners and the receipt store. A denial diagnostic proves
that no asynchronous mirror writer was created; primary completion alone would
not prove non-delivery. This is local runtime evidence, not Kubernetes admission
or delegated-traffic integration coverage. Delegation is covered by unit tests.
The historical backend-filter POST positive control used an explicit rule-level
RequestBodyBuffer and the committed-snapshot mirror path; those runs did not
verify unbuffered backend admission. The scenario now retains that control and
adds unbuffered fixed-length and chunked backend POSTs. Their separate admission
correction and current verification are recorded in
[body admission and committed snapshots](request-mirror-committed-body-snapshot.md).
Mixed invalid DSL phases check HTTP 500 without primary or mirror sends, then
valid-but-denied recovery and renewed authorization. DSL/WAF delta/full combinations
are covered by plugin-store unit tests.

Re-evaluate when adding mirror target types, additional plugin stages, changing
delegation inheritance, grant identity rules, or request-snapshot revocation policy.

## Resolution references

- `request-mirror-reference-grant-enforcement`: confirmed authorization defect;
  full HTTPRoute/delegation/EdgionPlugins scope and skip-mirror behavior approved.

## R3b field ownership and cold targets

Only a validated authored mirror loan can mint the ReferenceGrant query. HTTP
loans retain the original declaring Route and rule/backend/filter slot; plugin
loans retain the actual EdgionPlugins and request-entry ordinal. UID/generation
are borrowed where present, without adding transport metadata. Under delegation
each route authorizes its own fields and registers Service plus Grant-namespace
recovery under its own wake key.
Valid Service and cross-namespace dependencies precede the Grant read. Disabled
plugin mirrors still refresh denial and Grant-namespace dependencies, while Service
dependencies and status remain enabled-entry-only.

Gateway effective loans cannot mint Grant queries. Runtime construction computes
the canonical Service/section keys once; invalid and denied targets exit before
Session access, endpoint selection, task/body admission or writer creation. The
EP compiler uses the actual equal-length policy-masked owner and validates the
original mirror ordinal before cache reuse. No owner is synthesized from a store
key, and mirror references do not enter credential topology. Existing generation
cache, policy/LKG, cancellation, body-budget and separate mirror-client boundaries
remain in force. These are in-process boundaries, not a live delivery or conformance
claim. Historical September verification remains historical evidence.

R3b stable in-process verification (2026-10-08): Controller 927 tests passed,
Resources 1601 tests passed plus 27 doctests/10 ignored, and Gateway 2254 plugin
tests passed/17 ignored. The amended 297 original scoped tests are preserved,
with 14 additions and unchanged ignores; complete three-crate compiled lists have
no removals. Raw commands, failures, source/binary hashes and identity comparisons
were kept in local `tmp/resource-reference-contract/r3b-verification-20261008-r1`
evidence, which is no longer retained. Workspace gates and independent
actual-result review then passed, and R3b was committed as `16e7be6dc`.
