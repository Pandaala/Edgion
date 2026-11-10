---
name: select-backend-by-policy-dead-wrapper
description: Historical pass-through selector removal and the current shared captured Service selection boundary.
knowledge_type: regression-guards
---

# backends-F012 — canonical Service selection

The May 24, 2026 change removed `select_backend_by_policy`, a pass-through to the
old `select_from_endpoint_slice`. The Service/ServiceEndpointSlice branch names
in that review are historical; they do not describe supported native kinds now.

`CapturedServiceSelection::select` owns the current selection algorithm.
HTTP/gRPC capture it from the actual permitted compiled Service target, preserving
one Service source, checked section and membership-matched runtime per attempt.
The existing `select_service_endpoint` entry for L4, mirrors and custom consumers
loads their live projection and delegates to the same body. Native ordinary and
`gateway`/`controller`-mode ExternalName use this same selector; `system` mode uses
the OS resolver and the shared safe peer builder.

Do not add a duplicate selector or retain detached native wrappers solely for
tests. Re-evaluate real callers and failure mappings whenever strategy dispatch,
selection inputs or discovery ownership change. The old deletion and test results
are historical evidence, not current conformance proof.
