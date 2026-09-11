# B0307 — LinkSys Webhook Resolver Refresh

## Scope

- Baseline: `c0ebeefc`.
- Pipeline state: terminal; changed and committed after independent approval and all gates.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/link_sys/providers/webhook/resolver.rs`.
- Exact technical write scope is not approved during analysis. Existing `edgion-tests/**`, campaign records, unrelated modules and user-protected paths are read-only.

## Context and value gate

- External `b297af6e` threaded outbound diagnostics through the resolver. B0306 separately retained its WebhookKeyGet caller and identified two resolver-owned candidates: an obsolete compatibility argument and an avoidable path-override copy.
- The analyst must independently revalidate the resolver API and every caller, current-generation LinkSys lookup, Service/EndpointSlice selection, ReferenceGrant or namespace semantics, prepared webhook templates and override ownership, canonical outbound diagnostics/client/TLS/egress/caps, failure classification, allocations and deterministic tests before deciding change, examined-no-change or defer.
- Recheck the signature snippet in `skills/01-architecture/03-controller-gateway-link/webhook.md` and prior B0053 evidence. Existing integration tests remain frozen.

## Analysis result

- Analyst `b0306_webhook_key_get_analysis` formally re-read the resolver and traced every caller, manager generation, Service/EndpointSlice path, compiled request, canonical outbound client/TLS/egress/cap, diagnostics and tests at `c0ebeefc`.
- Decision: change, simple. The sole production caller always passes `None` for the obsolete `_forward_headers` migration argument; the removed consumer fields make the shim invalid under the pre-release policy.
- The resolver also allocates `path_override.to_string()` before learning whether overrides are allowed. Passing its existing borrow through `outbound_call` is lifetime-safe, while `CompiledRequest` retains the one owned copy only when the override is honored.
- Exact technical allow-list: resolver plus the WebhookKeyGet call site. Exact support doc: `skills/01-architecture/03-controller-gateway-link/webhook.md`, whose signature retains the obsolete argument and omits diagnostics.
- Preserve exact manager keys/current generations, namespace and endpoint semantics, Controller-owned reference authorization, shared outbound controls, value-free diagnostics and resolved/default/skip behavior. No integration change is needed.
- Wider LinkSys module-map and manager-rationale drift remains outside this exact batch; no new runtime defect or deferred task was confirmed.

## Implementation

- Implementer `b0307_webhook_resolver_implement` removed the obsolete compatibility argument from the resolver, its sole production call site and compile-only wrapper.
- The resolver now passes the existing `Option<&str>` directly through the async call; `CompiledRequest` retains an owned path only when overrides are allowed. The stale migration prose, false real-server note and duplicated local hop-by-hop test were removed.
- The support documentation now shows the exact signature and distinguishes request-construction inputs from caller-owned diagnostic emission.

## Independent review

- Reviewer `b0307_webhook_resolver_review`: LGTM with no Must Fix, Should Fix or deferred finding.
- It independently confirmed lifetime/ownership safety, complete caller cleanup, canonical replacement coverage for the removed test and unchanged LinkSys generation, namespace, endpoint, authorization, outbound and observability semantics.

## Verification

- Resolver tests passed 9/9, WebhookKeyGet 4/4, the focused webhook suite 53/53, current-generation diagnostics 1/1 and canonical hop-by-hop tests 4/4.
- Locked Gateway all-target check, formatting, Gateway no-tracing, agent-doc, diff and frozen-path checks passed during implementation/review. No integration or network test was run.
- Applied format plus locked workspace all-target check and Clippy passed. Agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff and frozen-path guards all passed; Clippy retained only existing warnings.

## Handoff

- Ledger/changelog updated: yes.
- Commit: `bfb8fe89`.
