# B0124 — Gateway API CORS Filter

## Scope

- Baseline: `486e8fea12bd870325b489839262729d14ecb22c`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Candidate owned file:
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/cors_filter.rs`
- Baseline blob: `708781e2a93cd8990d6d69dbe9b52f65c3855e1c`
- Candidate implementation allow-list: not frozen until analysis completes
- Forbidden files: every file outside an independently justified exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary

- Read the complete Gateway API CORS adapter, shared CORS plugin/schema ownership, route compilation/publication, preflight and response-header paths, tests, and relevant review knowledge.
- Preserve Gateway API semantics, canonical CORS validation/mutation, header safety, immutable publication, generic failure behavior, canonical access-log ownership, redaction, observability restraint, and response streaming.
- Existing integration tests remain frozen. Create a separate issue only if genuinely necessary new integration coverage cannot be represented without editing an existing file.

## Handoff

- Analysis: change. `build_cors_headers` has a proven maximum of six entries but allocates a heap `Vec` on every allowed response/preflight; the workspace already uses `SmallVec` for the same fixed-bound pattern. The public unchecked constructor is used only by four colocated tests while production already uses `try_new`, so removing it strengthens the validated immutable-publication boundary without behavior change.
- Complexity: simple; the exact proposed allow-list is the one target file including its colocated tests.
- Candidate plan: return a private `SmallVec<[(&'static str, String); 6]>`, remove the unchecked constructor, migrate its four test callers to `try_new`, and add a maximum-six-header ordering/value/non-spill regression. Preserve Gateway API v1.5 origin/credential/method/header semantics, local 204 ownership, denied pass-through, singleton set and `Vary` append behavior, PluginLog, immutable publication, and response streaming.
- Candidate wider findings for independent adjudication during this batch: backendRef-only CORS preflight is intercepted before backend selection and cannot emit its compiled headers; non-Kubernetes input validation is weaker than v1.5 admission; credentialed `exposeHeaders: ["*"]` semantics need a versioned decision. None is safe to fold into this representation-only edit.
- Integration disposition: no integration issue is needed for the local representation change. If the backendRef preflight ownership defect is confirmed, its separate task should also own a new integration-coverage issue for backend-only and mixed-backend selection rather than modify existing frozen tests.
- Freshness: analyzed at `486e8fea`; target blob matched `708781e2a93cd8990d6d69dbe9b52f65c3855e1c` and did not overlap B0121.
- Review: pending.
- Verification: pending.
- Commit: pending.
