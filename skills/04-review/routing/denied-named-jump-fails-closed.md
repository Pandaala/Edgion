---
name: denied-named-jump-fails-closed
description: Use when reviewing HTTP named-jump (DynamicInternalUpstream / Canary) backend selection findings; covers why only BackendNotFound may fall through to weighted selection and why plugins must validate through the compiled backend view.
status: fixed
finding_id: task-fail-closed-on-denied-l7-named-jumps
closed: 2026-09-05
---

# Denied L7 Named Jumps Fail Closed — FIXED

## Conclusion

An HTTP named jump resolves through exactly one matcher, `RouteRules::find_backend_by_name`,
and only its `EdError::BackendNotFound()` outcome may fall through to weighted selection.
`RefDenied` — and any other typed error — propagates to the caller, which records
`EdgionErrorCode::RefDenied` and ends the request. A plugin that publishes a named jump
validates its resolved target through the same matcher against the route's *compiled*
backend view before publishing, so a denied target follows the plugin's `onInvalid` policy
instead of becoming a jump the selector has to reject later.

Fix suggestions of the form "treat any named-lookup failure as a miss and fall back to
weighted selection", "scan `rule.backend_refs` directly for the target name", or "add a
second name matcher next to `find_backend_by_name`" are **not accepted**.

## Core Rationale

**1. Ok/Err collapse turned a policy denial into an undetectable misroute**

`pick_backend_ref` (`edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs`) used
`if let Ok(b) = find_backend_by_name(..)`, so `RefDenied` and a genuine miss were the same
outcome: weighted selection then picked a *legal sibling*. The request succeeded, went to a
backend the operator never asked for, and produced no access-log error code — even though
`backend_select_status` already maps `RefDenied` to `EdgionErrorCode::RefDenied` and the call
site already ends the request on `Err`. The typed error existed; it just never arrived. Only
`Err(EdError::BackendNotFound())` may be swallowed, because that is the documented
weighted-fallback contract for a target the route genuinely does not declare.

**2. A raw schema scan cannot see the Controller's ReferenceGrant decision**

`DynamicInternalUpstream` pre-validated its resolved target by iterating
`route_unit.rule.backend_refs` and comparing `backend_kind()` / `name` / `namespace`. That view
has no notion of `ref_denied`, so a denied cross-namespace target was judged valid, the jump was
published, and `DyeOutcome::Success` was recorded with the denied name — the configured
`onInvalid` policy could never fire. `find_backend_by_name` reads the compiled backend view,
which carries `schema.ref_denied`, and B0066 had already moved Canary onto it. DIU was the last
raw scanner **on the named-jump path** — `validate_endpoint_in_route` (DirectEndpoint) still
iterates `rule.backend_refs`, but it matches by endpoint address rather than name and carries its
own `ref_denied` check, so it is not a second name matcher.

**3. Kind matching did NOT change**

Matching moved from schema `backend_kind() == Service` to compiled `is_service()`, but these
select exactly the same set: `CompiledBackend::compile_target` maps `HTTPBackendRefKind::Service`
to `CompiledBackendTarget::Service` unconditionally, and `backend_kind()` already folds the whole
Service family (`ServiceClusterIp`, `ServiceImport`, `ServiceExternalName`, `EndpointSlice`, …)
into that one kind. Only `EdgionBackend`-with-a-port, `RouteDelegation` and `Unsupported` compile
to `Invalid`, and the old scan rejected those too. Do not read this change as a tightening, and do
not "fix" an `ExternalName` / `ServiceImport` / `EndpointSlice` jump target on that basis.

**4. What deliberately did not change**

Weighted fallback on a genuine miss, optional namespace matching, explicit selection of a
`weight: 0` backend (the documented "hidden backend" pattern — `find_backend_by_name` still does
not filter by weight), jump-lock (`SetJumpOutcome::Rejected`) behavior, and every `onInvalid`
response body and Dye outcome are unchanged. The one diagnostic addition is a distinct PluginLog
line for a denied target (`backend ref denied`) **in DIU only**, because the old shared
`backend ref not found` sent an operator hunting for a Service that exists. Canary was left on the
collapsed line (its code is B0066's, out of this task's scope) and is the remaining outlier — see
the residual below rather than re-reporting it.

## Known residuals — deliberate, not defects

- **`onInvalid: Fallback` on a denied target still reaches a legal sibling.** The plugin returns
  `GoodNext` without a jump, so ordinary weighted selection runs and may pick a sibling with a
  `Fallback` Dye outcome and no error code. That is what `Fallback` means: the operator opted out
  of the plugin owning the outcome. `Reject` (the default) is the fail-closed setting. Weighted
  selection landing on the denied slot itself still returns `RefDenied`, so a Fallback deployment
  can see both outcomes across requests — pre-existing weighted behavior, not introduced here.
- **`pick_backend_ref`'s new `RefDenied` arm is defense in depth.** Both in-tree jump publishers
  (DIU, Canary) now reject a denied target before publishing, so in-tree the arm is unreachable;
  `ctx.route_unit` is a single `Arc` fixed at match time, so there is no TOCTOU window either. It
  exists so a future jump producer cannot reintroduce the misroute, and so the typed error is
  never silently swallowed again.
- **The plugin's own rejection carries no `EdgionErrorCode`.** `PluginRunningResult::ErrResponse`
  records a Dye outcome and a PluginLog line, not an access-log error code, so a denied jump under
  `onInvalid: Reject` is observable as 403 + `on-invalid` rather than `RefDenied`. DIU's PluginLog
  line distinguishes the two causes (`backend ref denied` vs `backend ref not found`); **Canary
  still collapses both into `backend ref not found`**, and its own test pins that string, so an
  operator debugging a denied Canary profile gets the misleading line. That is a known gap, not an
  oversight to re-report: Canary's code belongs to B0066, and aligning it is a separate change.
  Giving plugin-local rejections a typed error code is a plugin-runtime change, also separate.
- **A denied ref shadows a legal same-name sibling.** With no `namespace` on the jump target,
  `find_backend_by_name` returns on the first name match, so `backendRefs: [{name: app,
  namespace: team-b}(denied), {name: app}]` fails closed rather than serving the second entry.
  The configuration is ambiguous by construction; fail-closed is the intended resolution.
- **A denied NON-Service slot is still the old shape.** `find_backend_by_name` skips every
  non-`is_service()` slot before it can read `ref_denied`, so a jump naming a denied
  `EdgionBackend`/`Invalid` slot returns `BackendNotFound`, logs `backend ref not found`, and under
  `onInvalid: Fallback` can still reach a legal sibling. Weighted selection, which evaluates
  `ref_denied` on whichever slot it picked, does not have this hole. Reachability is narrow —
  named jumps are not meant to address AI backends, and `is_permitted_ai_backend` gates the AI
  dispatch path separately — so this was left as-is rather than widening the matcher. If
  `find_backend_by_name` is ever widened to non-Service targets (see the trigger below), the
  denial check must move ahead of the kind filter in the same change.
- **Validation runs before the jump-lock check.** If an earlier plugin already locked a jump, DIU
  still evaluates `onInvalid` and can reject a request whose jump it would never have owned. This
  ordering is pre-existing (it already applied to a not-found target) and is shared with Canary;
  the fix widens which targets reach it. Reordering `onInvalid` behind `SetJumpOutcome` would make
  a denied reference silently ignorable and is not accepted.

## Fix Suggestions Not Accepted

- Fall back to weighted selection on any named-lookup error — reinstates the misroute; a denied
  reference must not be satisfiable by a sibling.
- Keep the raw `rule.backend_refs` scan and add a separate `ref_denied` check beside it — a
  second name matcher that will drift from `find_backend_by_name`. (This is the contract stated on
  `find_backend_by_name` itself and in `skills/01-architecture/02-gateway/07-backends.md`; it is
  not a row in the `05-unification-points.md` registry, which tracks mechanisms duplicated 3+
  times.) The denial predicate itself is shared: both matchers construct `RefDenied` through the
  one `ref_denied_error` helper.
- Let the plugin publish the jump and rely on the selector to reject it — the plugin's
  `onInvalid` policy (403 vs. fallback) and its Dye outcome are then bypassed, and the operator
  sees a 500 instead of the configured response.
- Reorder the plugin's `onInvalid` evaluation behind the jump-lock check — makes a denied reference silently ignorable whenever another plugin published first.
- Filter denied refs out of the compiled backend list at route-compile time — invalid and denied
  slots deliberately retain their declared traffic share; see
  [weighted-invalid-backendrefs-retain-traffic-share.md](weighted-invalid-backendrefs-retain-traffic-share.md).

## Re-evaluation Triggers

Re-open this decision only if:

- A named-jump producer legitimately needs to address a non-Service compiled target, which would
  require widening `find_backend_by_name` rather than reintroducing a local scan.
- Plugin-local rejections gain a typed `EdgionErrorCode`, at which point the denied-jump 403 path
  should carry `RefDenied` instead of only a Dye outcome.
- `EdError` gains another selection outcome that genuinely means "this route does not declare the
  target", in which case the fall-through arm must name it explicitly — never widen back to `_`.

## Reference Cases

- `tasks/todo/fail-closed-on-denied-l7-named-jumps.md` (closed 2026-09-05); sibling L4 work is
  tracked in `tasks/todo/l4-backend-ref-denied-fail-closed.md`, and the Canary half was B0066.
- Source: `edgion-gateway/src/routes/http/routes_mgr.rs` (`find_backend_by_name`),
  `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs` (`pick_backend_ref`),
  `edgion-gateway/src/plugins/http/dynamic_internal_upstream/plugin.rs`,
  `edgion-gateway/src/plugins/http/canary/plugin.rs`.
- Regression tests: `denied_named_jump_fails_closed_instead_of_selecting_sibling` (the denied ref
  must sit in a slot weighted selection would NOT pick first, or the test passes against the bug),
  `weighted_selection_reaches_the_legal_sibling_without_a_jump`,
  `missing_named_jump_falls_back_to_weighted_selection`,
  `allowed_named_jump_selects_the_named_backend`,
  `weight_zero_named_jump_is_selectable_but_never_weighted` (pg_upstream_peer);
  `diu_rejects_denied_backend_before_setting_jump`,
  `diu_falls_back_on_denied_backend_without_setting_jump`,
  `diu_selects_explicit_weight_zero_backend` (DIU plugin).
- End-to-end coverage is deliberately deferred to
  `tasks/todo/add-denied-named-jump-integration-coverage.md`.
- Docs: `docs/en/user-guide/http-route/filters/edgion-plugins/dynamic-upstream.md` and its
  `docs/zh-CN/` counterpart (`onInvalid` now also covers a ReferenceGrant-denied target).
