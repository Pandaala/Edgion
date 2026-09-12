# Move Plugin Execution Policy to EdgionGatewayConfig

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | in-progress |
| Type | refactor |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Migrate the Controller startup-only plugin execution policy to the
GatewayClass-scoped EdgionGatewayConfig resource. The user approved this direction
on 2026-09-10 and requested a plan, not implementation. The design must provide
dynamic policy publication across HTTP, stream, and TLS-route consumers while
preserving shared plugin state and preventing invalid DSL/WAF retention from
restoring an obsolete policy. On 2026-09-11 the user authorized completing the
detailed plan and starting implementation, explicitly without compatibility work.

On 2026-09-11 the user clarified that binding plugin execution policy to
EdgionGatewayConfig is a required configuration-ownership decision that enables
dynamic management. Its value does not depend on how often emergency disables
occur. The earlier retention-only HTTP fix does not satisfy the migration
requirement; implementation cost should guide scope control, not substitute for
that outcome.

## Plan documents

- [Design and impact](plugin-policy-edgion-gateway-config/01-design.md)
- [Implementation sequence and verification](plugin-policy-edgion-gateway-config/03-subtasks.md)
- [Optional Kubernetes class-list scope proposal](plugin-policy-edgion-gateway-config/04-kubernetes-class-list-proposal.md)

## Original finding and pre-migration evidence

An earlier HTTP-store correction prevented whole-runtime last-known-good retention
from discarding enabled Controller-stamped Block or Bypass additions, changes, or
removals in delta and full publication. That correction was intentionally narrow:
it preserved ordinary invalid DSL/WAF retention and did not move configuration
ownership, publish policy for stream or TLS-route consumers, or provide dynamic
GatewayClass-scoped policy. The historical evidence below predates that narrow
fix; this authorized configuration migration remains open.

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
  verification finish. Planning and implementation are authorized;
  Git commit and push are not.
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

## Current implementation status

The 2026-09-11 implementation now uses GatewayClass-scoped EGC publication across
HTTP and stream consumers, transports policy-independent AI admission facts, and
removes the Controller policy source and per-entry stamps. The original defect
was dynamically reproduced in four DSL/WAF × delta/full unit cases before the
migration; the updated HTTP store module now passes 36 scoped-publication tests.
The [implementation checkpoint](plugin-policy-edgion-gateway-config/03-subtasks.md#implementation-checkpoint--2026-09-11)
records focused results and outstanding failures without claiming final closure.

Independent code/knowledge review returned LGTM. Local shared dynamic integration,
workspace check/Clippy and project guards passed. Live Kubernetes single-managed-
class policy tests also passed (all HTTP stages, status changes, EGC deletion/
recreation and class rebinding), with unchanged Controller/Gateway Pod UID and
restart counts. Local and Kubernetes cache-residue validators passed in fail mode.

The two-class Kubernetes shared scenario cannot run with the existing mandatory
single `conf_center.gateway_class` filter. Its automatic K8s registration is withheld;
the local two-class scenario remains registered and passed. Expanding the startup
filter to an explicit nonempty `gateway_classes` list is a separate scope decision
presented to the user, not an implemented or implicitly enabled all-class mode.
The committed EGC CRD was used unchanged in Kubernetes; an unrelated EdgionTls
structural rejection needed a temporary TLS-only test copy. A supplemental KeyGet
coverage assertion also fails on its existing enum count. These limitations and
separate follow-ups remain recorded in the implementation checkpoint. This task
remains open; no commit or push has been made or authorized.

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
English/Chinese documentation are in scope. On 2026-09-11 the user authorized
the detailed plan and implementation without compatibility aliases. The
policy-specific integration scenario and its fixture/dispatch migration are
necessary supporting work; all unrelated scenarios remain frozen. No commit
or push is authorized.
