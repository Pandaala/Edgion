# D0024 — Unify Gateway Request-Template Runtime Policies

## Meta
- Status: needs-discussion
- Severity: high
- Discovered in: B0058, LinkSys template runtime audit
- Category: architecture-security
- Canonical task: [`tasks/todo/unify-gateway-request-template-runtime/`](../../../todo/unify-gateway-request-template-runtime/unify-gateway-request-template-runtime.md)

## Evidence
The Gateway has two live `${type:name}` runtime evaluator families. The LinkSys engine separately implements missing-as-empty and bounded missing-as-error evaluation and cannot resolve Secrets. Outbound request rendering duplicates segment walking and resolution to add controller-resolved Secrets, JSON-variable-only escaping, and different empty-`RemoteIp` handling. Consumers also require distinct output caps and failure classes. Local cleanup in the two LinkSys files would preserve this drift instead of establishing the required canonical mechanism.

The audit also found that upstream authority is a destination template but currently admits client-controlled header, query, cookie, path, URI, request-path, request-URI, and method variables. It also admits `secretRef` despite having no resolved-Secret capability. Separately, outbound template rendering returns a bare `String`, so a header assembled from resolved Secret material has no provenance that can force `HeaderValue::set_sensitive(true)`. The wider task must apply restricted-variable admission and establish Secret-derived header sensitivity as part of the canonical typed policy design.

## Why deferred
The correction is complex, design-first, and cross-cutting across resource validation, the Gateway evaluator, six consumer families, controller Secret readiness verification, error/failure contracts, bounded allocation behavior, and canonical documentation. An isolated change would either alter established consumer semantics or create a third policy path.

## Existing-task adjudication
This finding absorbs and supersedes [`upstream-authority-bounded-template-rendering.md`](../../../todo/upstream-authority-bounded-template-rendering.md). That narrow task shares the same bounded evaluator seam and cannot run independently without duplicating the canonical mechanism; its 63/64-byte, DNS-label, suffix, and pinned-resolution acceptance remains mandatory in the new task.

## Integration-test impact
Existing integration tests remain frozen. The identified policy matrix is covered most precisely with resource, controller-handler, and Gateway unit tests, while existing authority integration coverage already exercises Controller-to-Gateway wiring. No separate integration-test issue is needed unless implementation later exposes a cross-process capability/status behavior that cannot be proven locally.
