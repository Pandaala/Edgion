# Verify the declared managed AI APIs against real providers

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-10-12 |
| Status | pending |
| Type | feature |
| Priority | P1 |
| Tier | Future verification; previously completed scoped checks remain complete |
| Dependencies | Current supported native APIs and existing test harness; no optional roadmap feature is required |

## AI guide summary

Finish the explicitly bounded remaining verification scope below without reopening
completed local implementation or inferring broader acceptance from historical passes.

## Completed foundation and evidence limits

Existing local verification on 2026-10-07 covered 16 native SDK fixture cells,
61 static cases, seven hot-update scenarios and strict residue. Supplemental
manual testing on 2026-10-12 confirmed normal Codex/Claude Code subscription
conversation forwarding through ordinary HTTP routes. Both clients returned a
complete list through 500. Deliberate active cancellation was not tested.
That supplemental run reused binaries from 958f8fe956af4a7f59610fe4dc828feaa43c84d6;
it did not exercise managed AiProxy, Secret-backed API credentials or the public
SDK acceptance matrix. These are historical boundaries, not fresh current-HEAD
acceptance. Preserve the working local harness; fill only the remaining evidence.

## Requirements and scope

- Freeze the support matrix for native OpenAI Chat, OpenAI Responses and Claude
  Messages creation; use the current supported client dialect for each. Optional
  conversion is a separate conditional matrix, not implicit native acceptance.
- Prepare one owned loopback Controller/Gateway topology with actual managed AI
  routes and isolated backend credentials. SDK clients connect only to that gateway.
- Approve non-secret endpoints, exact model IDs, Secret/key-field references and
  maximum spend before execution. Never ask for credentials in chat or put them
  into committed fixtures or debug output. Check current official API/model rules.
- Initial proposed scope: JSON/SSE x observation off/provider x three APIs, at most
  12 serial attempted calls. Failures consume a slot; retries and fallback are off.
  Use a short fixed prompt and a supported 64-token output limit. Limits on requests
  and output do not enforce a hard invoice ceiling; approve spending separately.
- Responses uses store=false/background=false without conversation history or hosted
  tools. Fix HTTPX redirect/proxy behavior and whole-request deadlines, including
  stream consumption, with the existing canonical deadline helper.
- Implement a live-specific driver; local fixture assertion scripts are not live
  drivers. Record only fixed outcomes/status, timings, versions and numeric usage.
- Revalidate local/static/dynamic tests appropriate to current production changes.
  Prove cancellation, once-only finalization and all-attempt accounting with bounded
  fixture faults; any provider-specific live cancellation test needs its own scope.
- Clean routes, owned services and private credential copies; run strict residue.
  Record PASS/FAIL/SKIP per cell and narrow support claims for unexecuted cells.

### Out of scope

Subscription-login token extraction, uncontrolled paid load testing, provider-state
emulation or claiming every compatible endpoint passed from one OpenAI endpoint.

## Acceptance criteria

- [ ] Every claimed managed native cell has current SDK/provider evidence or its
  support claim is explicitly removed/narrowed.
- [ ] JSON/SSE framing, observation off/provider and supported usage-quality semantics
  match the selected provider responses without payload/credential disclosure.
- [ ] Unsupported operations and failed cells remain visible; a subscription-client
  ordinary proxy smoke cannot substitute for this managed API evidence.
- [ ] Owned cleanup, strict residue and appropriate current-code regression gates pass.

## Source entry points

- [edgion-tests/src/client/suites/edgion_backend/single_provider.rs](../../../edgion-tests/src/client/suites/edgion_backend/single_provider.rs)
- [edgion-tests/src/client/suites/edgion_backend/anthropic_provider.rs](../../../edgion-tests/src/client/suites/edgion_backend/anthropic_provider.rs)
- [edgion-tests/src/client/suites/edgion_backend/resilience.rs](../../../edgion-tests/src/client/suites/edgion_backend/resilience.rs)
- [edgion-gateway/src/ai/runtime/dispatch.rs](../../../edgion-gateway/src/ai/runtime/dispatch.rs)

## Independent implementation boundaries

This is future work, not authorization to implement or call a paid provider now.
Revalidate the current source and applicable review knowledge when starting; code
pointers below are investigation entry points, not verified completion claims.

- Edgion is a proxy-layer gateway. Preserve client-owned model choice and native
  payloads. Keep conversion and usage observation explicit; observation defaults off.
- Reuse canonical credentials, auth/key extraction, routing, snapshots, retries,
  deadlines, Redis limits, attempt accounting and final access logging. No agent
  runtime, hosted tool execution, MCP/A2A gateway or new parallel infrastructure.
- Main upstream responses stream. No response BodyStorage, disk spool, unbounded
  aggregation or second SSE window. Semantic windows use bounded RawWindow. The
  existing selected unary Chat-to-Anthropic converter has its own strict memory,
  complexity, admission, deadline and precommit-error contract; do not extend that
  exception to another converter without a separate reviewed design.
- Compile policy before request processing. Preserve cancellation cleanup,
  all-attempt accounting and Controller serial workqueue discipline.
- Gateway traffic diagnostics belong to canonical access logs and bounded metrics;
  no request-path tracing, payload/credential logging or identity/model metric labels.
- Any operator-facing schema change needs CRD coverage; internal resolved fields
  retain schema hiding, conf_sync serialization and secret redaction.

## Knowledge and verification

Read [project rules](../../../AGENTS.md), then the [knowledge index](../../../skills/SKILL.md)
and [design rules](../../../skills/00-design-rules.md). Load the relevant original
[AI/backend architecture](../../../skills/01-architecture/05-resources/22-edgion-backend.md),
[review index](../../../skills/04-review/SKILL.md) and
[testing index](../../../skills/05-testing/SKILL.md) on demand. Those project
sources remain authoritative; this task is self-contained as a work specification.

Use a reviewed plan for complex implementation. Add meaningful targeted unit,
integration and hot-reload tests; preserve ordinary HTTP/native/SSE controls and
strict residue. Put unit bodies under the owning crate's tests/unit tree. Run the
applicable project checks and update current capability/user documentation.
Run cargo test --all only when explicitly requested. Paid calls require a concrete
endpoint/model/credential-reference/call-count/budget scope authorized for that run.
