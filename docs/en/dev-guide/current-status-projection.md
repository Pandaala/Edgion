# Current-status projection design and implementation plan

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-21 |
| Status | implemented and verified |
| Type | refactor |
| Priority | P1 |
| Issue | current-status-projection |

## Scope and authorization

The user approved keeping complete status in ServerCache while generating a
separate typed currentStatus on the Controller cold path for Gateway decisions.
Implement this consistently across status-bearing resources and existing status
consumers. This projection change did not alter controllerName, election topology,
or class admission. The subsequent
[GatewayClass reporting correction](../../../skills/04-review/status/regression-guards/gatewayclass-reporting-and-publication.md)
separates reporting from publication under a distinct-deployment identity contract.

## Design

Each of the 17 Controller status-bearing resource specs gains an internal
`current_status: Option<NativeStatus>`, serialized as `spec.currentStatus`, with
`serde(default, skip_serializing_if = "Option::is_none")` and `schemars(skip)`.
The four partitioned status resources use their native payload type rather than
the multi-controller envelope. No operator CRD field or user-facing knob is added.

The two views have independent purposes:

- `status` retains the complete logical status, including foreign observations,
  for persistence and diagnostics. It is not a data-plane authorization input.
- `spec.currentStatus` is recomputed by the Controller for the current deployment
  and published atomically with that resource generation. It is the sole source
  for existing Gateway decisions based on resource status.

The processor clears incoming currentStatus before handlers, retains a complete
observed status snapshot, and uses existing native working status for handlers.
After computing local/persisted projections it assigns the local currentStatus
and restores the complete logical status. Failed envelope merge or capacity
checks preserve the original full status and the independently computed local
result; suppressed persistence must not erase both views. No processed spec is
serialized/deserialized to change either status field. Wire-only null tombstones
remain confined to the persistence payload after the cache object is prepared.

Current status must be ready for Gateway attachment-proof finalization. Route
current status must be filtered using the Controller-authored identity, including
FS/etcd; incoming identity fields cannot establish ownership. Finalization runs
once. Cache readers inside the Controller must use the local view where their
operation requires deployment-local status rather than API history.

Gateway route compilation and plugin publication share the same selector. A
missing currentStatus, missing owned parent entry, missing/empty identity, or
ambiguous duplicate entry cannot fall back to full status or optimistic approval.
Gateway listener decisions and ACME challenge publication also use currentStatus.
Resources whose runtime behavior already depends on resolved spec and independent
validation gain no unrelated new status-based admission gate.

## Implementation and verification

1. Add typed fields, serde/schema guards, and update resource construction sites.
2. Extend the existing typed status adapter and processor publication sequence;
   migrate Controller local cache consumers and shared listener/challenge helpers.
3. Migrate all direct and indirect Gateway status consumers, including five Route
   families, plugin publication, listener eligibility, ACME, and BackendTLS policy
   equivalence. Preserve existing ownership proofs and resolved spec boundaries.
4. Test full/current contradictory outcomes, preservation of foreign API status,
   forged currentStatus replacement, missing-current rejection, typed envelope
   projection, FS/etcd projection, sidecar preservation, and full/incremental
   publication changes. Update existing fixtures to represent processed resources
   rather than introducing production fallback for tests.
5. Update canonical status/Gateway transport knowledge and bilingual operator
   diagnostics documentation. Obtain independent code and knowledge review.
6. Run targeted tests, CRD/schema guards and repository-required formatting,
   workspace check/Clippy, agent-doc, SSA, metrics, Pingora source and transform
   guards. Do not claim live conformance without an actual run.

Complexity is complex; regression risk is medium. Costs are additional cold-path
status copies and serialized payload size. No per-request lookup or allocation
is introduced by the split. No commit or push is authorized.

## Review and ownership

Resource/Controller and Gateway changes were implemented in separate file scopes.
An independent read-only reviewer approved the design and reviewed the code and
canonical knowledge. Review found and corrected native FS/etcd foreign-status
filtering for EdgionTls and both policy families. This document retains the
implementation plan, not an additional active issue. The reporting correction
linked above records the separate class writer/coverage mechanism.

## Verification notes

Agent-doc validation initially failed on a pre-existing reference from
`skills/04-review/listeners/regression-guards/egc-runtime-snapshot-publication.md`
to missing `tasks/todo/http-keepalive-request-limit-counting.md`. The dangling
link is now an explicitly historical path; no accounting fix is claimed.
Agent-doc validation subsequently passed.

Final verification on 2026-09-21:

- Controller: resource_processor 821 passed; current-status filter 18 passed;
  ACME 33 passed (overlapping subsets, not an aggregate count).
- Gateway: routes 743, publication 21, listener index 17, ACME 17,
  BackendTLS policy 25, runtime matching 48, TLS store 22; all passed.
- Resources: current_status_transport 3, plugin_crd_type_coverage 6,
  structural CRD coverage 19 and plugin condition coverage 8; all passed.
- cargo fmt --all and final format check, workspace all-target cargo check and
  Clippy, agent docs, SSA force, metrics inventory, Pingora source policy, and
  Gateway API transform contract checks passed. Two unrelated existing Clippy
  warnings remain in Kafka and DSL gRPC test code.
- Independent code/knowledge review and final incremental review: LGTM.
- GatewayDynamic now asserts complete/current transport in initial and updated
  phases, and was compiled by the workspace check. No live dynamic, Kubernetes,
  etcd, full integration or conformance execution is claimed.

No commit or push was performed. Complete status is retained; currentStatus is
an internal typed projection, not an operator-configurable field or position
pointer. GatewayClass reporting is covered by the separate correction linked above.
