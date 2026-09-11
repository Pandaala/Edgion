# Move Plugin Execution Policy to EdgionGatewayConfig

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | refactor |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Migrate the Controller startup-only plugin execution policy to the
GatewayClass-scoped EdgionGatewayConfig resource. The user approved this direction
on 2026-09-10 and requested a plan, not implementation. The design must provide
dynamic policy publication across HTTP, stream, and TLS-route consumers while
preserving shared plugin state and preventing invalid DSL/WAF retention from
restoring an obsolete policy. Implementation remains pending detailed plan approval.

## Plan documents

- [Design and impact](plugin-policy-edgion-gateway-config/01-design.md)
- [Implementation sequence and verification](plugin-policy-edgion-gateway-config/03-subtasks.md)

## Problem and evidence

`compile_plugins` computes `retain_previous_on_invalid_update` from invalid DSL/WAF
construction. Policy markers do not change that flag. `can_retain_previous` checks
the flag, previous activation, and cross-namespace mirrors, but not policy changes.
Both `apply_delta` and `replace_all` can retain the entire older CompiledPlugins.
Consequently an incoming Block or Bypass can be compiled correctly and then never
published. This is confirmed call-chain evidence, not a newly executed reproduction.

A Controller restart triggers Gateway relist. `list_and_reset` calls
`publish_list_payload`, then `CacheData::reset`, `PluginStore::full_set`, and
`replace_all`. Resetting the resource cache does not first empty PluginStore.
The existing `invalid_full_snapshot_retains_last_known_good_but_drops_absent_keys`
test explicitly asserts full-set runtime retention; it was inspected, not rerun
during planning.

The original policy design requires Block to precede rejection for other invalid
plugins. The operator guide also requires restricted plugins not to execute.
The WAF reload overview does not establish an exception to either guarantee.
Policy-removal behavior under invalid updates was not previously specified as
precisely as the migration proposal below.

## Requirements

- Use `EdgionGatewayConfig.spec.pluginPolicy` as the sole operator-facing source;
  remove Controller `validation.plugin_policy` without aliases or dual-source fallback.
- Scope policy by GatewayClass parametersRef, with no process-wide aggregation
  across unrelated EdgionGatewayConfig resources.
- Publish policy updates without restarting either binary. All three existing
  policy families must migrate together.
- A current policy must govern any retained plugin implementation. Preserve
  the existing resource-owned state of allowed plugin instances across scopes.
- Explicitly resolve missing/deleted/invalid policies, removal/relaxation, new
  requests on existing connections, Controller admission, and status ownership.
- Keep this task open until implementation, independent review, and required
  verification finish. The current authorization is planning only.
- Trace Controller policy stamps, Gateway compilation and `retain_previous_on_invalid_update`, policy sealing, and both `PluginStore::apply_delta` and `replace_all`.
- Reproduce an existing activatable unblocked generation followed by a new Block stamp plus an unrelated retainable invalid DSL/WAF entry. Observe the actually published runtime.
- Verify intended precedence between operator policy and whole-resource retention from original requirements and review knowledge; do not infer it solely from current code.
- Include policy removal, Bypass, initial publication without a previous generation, and corrected subsequent updates in the decision matrix.
- If a defect is confirmed, propose one immutable publication fix with deterministic unit regressions, preserving access-log/metric ownership and static generic rejection bodies.
- Original investigation constraint: leave existing integration tests unchanged.
  The migration plan proposes a narrow exception for the existing plugin-policy
  scenario and its dispatch/fixtures, because the current canonical runner invokes
  a test of the Controller setting being removed. This exception is part of the
  detailed plan approval, not permission already exercised.

## Acceptance criteria

- HTTP, stream, and TLS-route execution policies all use the EGC source, with
  no Controller startup policy, serialized stamp, alias, or dual-source fallback.
- EGC changes dynamically govern new admissions and retained implementations,
  including full_set/relist, without restarting Controller or Gateway.
- Independent evidence reproduces the original gap and verifies its correction;
  multi-scope state sharing and construction boundaries satisfy the design.
- The lifecycle, attachment, AI-admission, shared-listener and regression matrices
  in the linked plan are implemented and verified with independent review.
- Required project checks and replacement integration/CRD coverage pass before
  closing this task. Planning alone cannot satisfy migration acceptance.

## Scope and authorization

The user accepted expanding this investigation into the EdgionGatewayConfig
migration plan. Schema/CRD, Controller validation and policy-dependent route
handling, Gateway publication and consumers, focused tests, and corresponding
English/Chinese documentation are in the planned scope. Existing integration
tests remain untouched during planning. A specific migration-only exception is
proposed in the plan; all unrelated scenarios remain frozen. No production code,
CRD, existing test, or runtime
configuration is changed by the planning deliverable. No commit or push is authorized.
