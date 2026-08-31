# Unify Gateway Request-Template Runtime Policies

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | pending |
| Type | refactor |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Replace the two live Gateway `${type:name}` runtime evaluator families with one compiled-segment walking and resolution seam whose consumer supplies explicit missing-value, escaping, Secret-capability, and output-byte-budget policies. This is a design-first cross-cutting change: the consumers intentionally have different failure contracts, destination templates need restricted-variable admission, and bounded output must account for aggregate UTF-8 bytes and JSON escape expansion without claiming that source-value allocation is bounded.

## Requirements

- Keep one canonical compiled-template segment walker and runtime resolver seam. Do not add another parser, recursive expansion pass, or consumer-local variable switch.
- Preserve compile-once behavior and treat `${...}` text produced by a resolved value as literal data.
- Model missing-value policy explicitly:
  - empty for ctx-set, dye, upstream authority, ordinary outbound templates, and timeout fallback;
  - error for proxy-rewrite JSON `valueTemplate`.
- Model Secret capability explicitly. Only controller-resolved outbound-request surfaces may resolve `secretRef`, using the existing readiness, dependency, redaction, and sensitive-provenance contract. Ctx-set, dye, proxy rewrite, and upstream authority must reject `secretRef` during admission; never add a generic request-session Secret lookup.
- Model escaping explicitly. JSON mode escapes substituted variable bytes only; literal template segments remain verbatim.
- Model an optional output budget with overflow-safe checked UTF-8 byte growth before every append, including JSON escape expansion and aggregate multi-segment output.
- Do not claim total allocation is bounded while variable accessors return owned `String` values. A broader borrowing change requires separate evidence and explicit scope.
- Treat upstream authority and timeout templates as security-sensitive destination/deadline surfaces. Admission must allow only the established trusted variables (`ctx`, `remoteIp`, and matched HTTP/gRPC route metadata) and reject client-controlled header, query, cookie, path, URI, request-path, request-URI, and method values. Upstream authority must also reject `secretRef` because it has no resolved-Secret capability.
- Preserve each consumer's error and failure contract: authority `RenderEmpty` versus `InvalidLabel`; proxy rewrite missing value as 400 and oversized output as 413; timeout invalid, empty, or below-floor fallback plus its existing metric; outbound required-origin handling; no fifth failure-policy shape.
- Never emit rendered values, Secret material, client-controlled source values, or complete sensitive destinations in tracing, errors, or logs. The canonical render result must establish and carry whether any substituted segment came from `secretRef`; every outbound header sink must call `HeaderValue::set_sensitive(true)` before insertion, cloning, or transport for any Secret-derived value, including composed values and arbitrary custom header names.
- Add no request-path parsing, blocking lock, unbounded response aggregation, or new metric unless the existing outcome vocabulary is demonstrably insufficient.

## Scope

### In scope

- Canonical compiled-template/segment and variable-policy ownership in `edgion-resources`, including a reviewed common-module move and all required re-exports.
- Admission and capability validation in:
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/ctx_set.rs`
  - `edgion-resources/src/resources/edgion_plugins/dye.rs`
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/proxy_rewrite.rs`
  - `edgion-resources/src/resources/upstream_authority.rs`
  - `edgion-resources/src/resources/common/outbound_request.rs`
  - `edgion-resources/src/resources/common/external_http.rs`
- The canonical Gateway evaluator and facade in `edgion-gateway/src/link_sys/template/`.
- Migration of ctx-set, dye, upstream-authority, proxy-rewrite JSON `valueTemplate`, outbound request rendering, and the outbound timeout call boundary.
- Verification of every ordinary outbound-request consumer and header sink, including Webhook, ForwardAuth, Guardrail, and AI Guard, even when the shared `CompiledRequest` migration requires no consumer-local edit.
- Controller Secret resolution/handler support only if the selected typed capability representation requires it; otherwise verify the existing `ResolvedSecrets` readiness and dependency contract without changing it.
- Canonical contributor/feature documentation for each consumer's missing, escaping, Secret, destination, and budget policy.

### Out of scope

- Proxy-rewrite's separate `$uri`/`$arg`/capture rewrite language.
- Adding operator-configured limits merely to make the evaluator API uniform. Any new schema field requires separate product evidence, CRD/schema review, and explicit scope.
- A generic request-path Secret store or Secret lookup API.
- Changing the current consumer-visible failure classes, fallback behavior, or metrics.
- Modifying existing integration tests.

## Acceptance criteria

- Exactly one runtime segment walk resolves all migrated `${type:name}` consumers with explicit, typed consumer policy.
- All variable kinds are covered for present, absent, and empty values; empty `remoteIp` follows the selected consumer policy rather than evaluator-family accident.
- Secret tests cover admission denial, allowed resolved and absent values, readiness/dependency behavior, redacted diagnostics, and render-result provenance. Every Secret-derived header template, including `Bearer ${secretRef:...}` and arbitrary custom header names, produces a `HeaderValue` with `is_sensitive() == true`; equivalent non-Secret templates remain false.
- JSON tests distinguish literal segments from substituted bytes and cover quotes, backslashes, control characters, and escape-expansion budget accounting.
- Budget tests cover zero, exact limit, limit plus one, and cumulative multi-segment growth; the output buffer never grows beyond its cap.
- Injected `${...}` text remains literal and no consumer adds a second expansion pass.
- Upstream-authority admission rejects every client-controlled variable and `secretRef`; accepted trusted variables retain current route/context semantics.
- Authority renders exactly 63 bytes successfully, rejects 64 or more before output growth crosses the cap, and retains missing-as-empty, fixed suffix, lowercase normalization, separator/hyphen rejection, IP-literal rejection, Service anchoring, and pinned resolution.
- Proxy rewrite retains missing-as-400 and oversized-as-413 behavior; timeout retains fallback and metric behavior; outbound required-origin behavior is unchanged.
- Focused resource, Gateway, and controller tests cover the policy matrix. Existing integration tests remain untouched; create a separate integration task only if implementation discovers a cross-process capability/status behavior that unit and handler tests cannot prove.
- Formatting, workspace check, Clippy, documentation validation, SSA, metrics, Pingora source, Gateway no-tracing, and integration-freeze guards pass.

## Document index

| Document | Status | Notes |
|----------|--------|-------|
| `unify-gateway-request-template-runtime.md` | done | Analysis boundary and acceptance contract |
| `00-context.md` | not created | Create before implementation |
| `01-design.md` | not created | Required before implementation |
| `02-implementation.md` | not created | Create after independent design approval |
| `03-subtasks.md` | not created | Required when the multi-module design is approved |

## Affected modules

| Module | Impact |
|--------|--------|
| `edgion-resources` template/resource validation | Own canonical typed policies and restricted admission |
| Gateway LinkSys template runtime | Become the single evaluator seam |
| Gateway request-template consumers | Migrate without changing consumer-visible semantics and establish Secret-derived header sensitivity |
| Controller Secret resolution | Verify or minimally support typed Secret capability |
| Feature/contributor documentation | State the exact per-consumer contract |

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-31 | Absorb `upstream-authority-bounded-template-rendering.md`. | A separate authority evaluator design would overlap the same canonical bounded seam and violate cross-cutting unification. |
| 2026-08-31 | Keep existing integration tests immutable and require no new integration issue now. | The identified missing, escaping, Secret, budget, and failure contracts are precisely testable in resource, handler, and runtime unit tests; the current authority integration path already covers cross-process wiring. |
