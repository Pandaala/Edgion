# Business Mesh Task Index

- Created: 2026-09-25
- Status: pending implementation
- Parent: [Overall specification](01-spec.md)

## Execution order

Tasks 01-07 form the initial delivery sequence. Task 05 demonstrates the first
automatically configured TLS/mTLS path; task 06 completes automatic leaf renewal.
Task 08 is a separate investigation and does not gate the initial delivery.

Current research priority is task 08's [Istio + Edgion integration](04-istio-edgion.md).
It can proceed without tasks 01-07 or an Edgion-owned issuer. Their sequence remains
available for the independent business TLS approach; implementation is pending.

| ID | Task | Depends on | Status | Output |
| --- | --- | --- | --- | --- |
| 01 | [Existing-capability baseline](tasks/01-existing-capability-baseline.md) | None | pending | Not produced |
| 02 | [Controller management contract](tasks/02-controller-management-contract.md) | 01 | pending | Not produced |
| 03 | [Persistent business CA](tasks/03-persistent-business-ca.md) | 02 | pending | Not produced |
| 04 | [Scoped leaf issuance and delivery](tasks/04-scoped-leaf-issuance.md) | 03 | pending | Not produced |
| 05 | [Automatic TLS/mTLS policy reconciliation](tasks/05-automatic-tls-policy.md) | 04 | pending | Not produced |
| 06 | [Automatic renewal and recovery](tasks/06-renewal-and-recovery.md) | 05 | pending | Not produced |
| 07 | [Integration acceptance and operations guide](tasks/07-integration-and-operations.md) | 06 | pending | Not produced |
| 08 | [Ambient feasibility and prototype decision](tasks/08-ambient-feasibility.md) | None for integration research | in-progress | [Initial assessment](04-istio-edgion.md); prototype pending |

## Shared execution rules

- Start with task 01 for the independent TLS track, or task 08 for Istio integration.
  Complete one coherent task at a time and update its status,
  evidence, and this index. The task list does not claim implementation is done.
- Read repository AGENTS.md and the knowledge index, then load applicable
  architecture, resource schemas, and review knowledge progressively. Revalidate
  earlier source observations before editing.
- Before feature implementation, record the relevant context and concrete design
  in the task document or a companion document when needed. Do not create empty
  document sets. Task 02 owns the cross-cutting configuration contract.
- Reuse existing Gateway resources and TLS runtime. No new Mesh CRD or binary is
  assumed; document a concrete need before introducing one.
- Preserve controller workqueue discipline. Signing, renewal timers, and API work
  must not block resource handlers; feed changes through existing watch/requeue
  paths. Do not add a second queue or worker for an existing resource kind.
- Restrict generated-resource ownership explicitly. Forced SSA is required by the
  repository but does not authorize stealing user-managed fields.
- Keep business CA keys separate from conf_sync credentials and out of data-plane
  sync. Apply canonical secret redaction and schema invariants to new fields.
- Use targeted tests that establish lifecycle and failure behavior. Follow project
  unit-test locations and integration conventions. Historical tests and resource
  status alone are not handshake or convergence evidence.
- Run applicable project gates for implementation changes. Run the full test suite
  only when explicitly requested. Documentation-only changes need link/content
  checks. Do not commit, push, or deploy real credentials without task authority.
- If deployment execution lacks an environment, prepare reviewable configuration
  and test commands, record the missing evidence, and do not mark validation done.

## Delivery boundaries

The first release target is an explicitly reached business Gateway with
Controller-managed certificates and existing TLS-capable receiving endpoints.
Certificate automation does not turn a plaintext application into a TLS endpoint.
Backend delivery/reload and safe removal must be designed before claiming complete
automation. Ambient adds a separate transport and identity scope.
