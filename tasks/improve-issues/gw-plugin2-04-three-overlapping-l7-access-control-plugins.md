# gw-plugin2-04: Three overlapping L7 access-control plugins (IpRestriction, RequestRestriction, RequestAccessPolicy) with three different policy models

- **Category**: over-design
- **Severity**: P3 (direction)
- **Scope**: large-direction
- **Location**:
  - `edgion-gateway/src/plugins/http/ip_restriction/plugin.rs:60-114` (allow/deny groups + `defaultAction`)
  - `edgion-gateway/src/plugins/http/request_restriction/plugin.rs:118-170` (one `ConditionSet` + required global `mode` Allow/Deny)
  - `edgion-gateway/src/plugins/http/request_access_policy/plugin.rs:326-537` (named profiles → AND of `requiredRuleGroups` → OR of `anyOfRules` over DirectPeerIp/RemoteIp/Url + supplemental `ConditionSet`; implicit deny; ConfigData-selected active profile)
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/request_access_policy.rs:40-205`
  - Also overlapping: `geoip_location/plugin.rs` deny rules and the per-entry `conditions` on every plugin

## Summary
Edgion ships three request-stage plugins whose only job is to answer "allow or deny this request".
Each has its own composition model, deny-response shape and failure semantics.

- **IpRestriction** follows the project-wide canonical shape: allow/deny lists plus `defaultAction`
  (CLAUDE.md "Cross-cutting unification rules" #11).
- **RequestRestriction** uses one global polarity (`mode: Allow|Deny`) over a `ConditionSet`. IP
  conditions are also available through it.
- **RequestAccessPolicy** adds a third model: a profile switch selected at runtime from ConfigData,
  AND-of-OR rule groups, IP rules for both `DirectPeerIp` and `RemoteIp`, a URL index (radix + regex,
  host sets, URL allow-lists imported from ConfigData) and supplemental conditions. Anything that
  does not match is denied implicitly.

The IP and condition predicates are shared, but the three policy layers above them are not. Each
covers most of what the others express.

## Current behavior (evidence)
- Different failure semantics for the same class of misconfiguration:
  - RequestAccessPolicy: `init_failed` → deny with the configured status (`plugin.rs:497-500`).
  - RequestRestriction: invalid policy → fixed 500 JSON body (`request_restriction/plugin.rs:120-133`).
  - IpRestriction: runtime decisions only. A missing client IP is denied unless the default is Allow
    (`ip_restriction/plugin.rs:69-76`).
- Different polarity models: lists + `defaultAction` (canonical rule #11) vs a global `mode` vs
  implicit-deny required groups. RequestAccessPolicy has no `deny` list and no `defaultAction`, so
  "allow `/api/*` except `/api/admin` from these IPs" cannot be written in it. Operators then chain
  two plugins, which is the workaround the RequestRestriction knowledge page names as a re-evaluation
  trigger.
- RequestAccessPolicy reimplements path matching (`RuntimeUrlIndex`, `plugin.rs:66-189`) with its
  own radix canonicalization (`canonical_radix_path`) and regex list, separate from route matching and
  from the `Condition` path accessors.
- Runtime profile switching through ConfigData (`active_profile`, `plugin.rs:470-490`) is a feature
  no peer gateway ACL offers inside the policy object.

## Industry comparison
- **Envoy RBAC**: one filter, `action: ALLOW|DENY|LOG`, policies are OR-ed, each policy is
  permissions × principals. Allow-with-exceptions is written as a DENY filter in front of an ALLOW
  filter of the same type. One model covers IP, path, header and JWT principal.
- **Istio AuthorizationPolicy**: one CRD, CUSTOM → DENY → ALLOW evaluation order, implicit deny only
  when an ALLOW policy exists. Profile switching is done by changing the policy, not with an
  in-policy selector.
- **Kong**: `ip-restriction` (allow/deny) plus `acl` (consumer groups) and `request-termination`.
  These are small and orthogonal rather than overlapping.

## Impact
- Operators must learn three policy semantics and choose among them. Mixed use on one route yields
  ordering-dependent outcomes with inconsistent deny bodies and status codes.
- Every cross-cutting change has to be made three times: trusted-IP model, deny-body redaction,
  status validation, observability fields, ConfigData partial-validity rules (see
  `docs/en/user-guide/http-route/filters/edgion-plugins/conditions.md:123`, which already special-cases
  RequestAccessPolicy).
- It contradicts the project's own canonical access-policy shape (#11) in two of the three plugins.

## Suggested direction
Converge on one access-control plugin with the canonical model:
`deny: [ConditionSet…]`, `allow: [ConditionSet…]`, `defaultAction`, deny-first. IP lists remain
`IpGroup` references and are expressed as conditions. URL/host matching becomes a condition kind
backed by the shared path matcher.
- Keep IpRestriction as a thin preset (or alias) for the IP-only case if the ergonomics matter.
- Fold RequestRestriction into the unified plugin; its `mode` maps to allow-only or deny-only.
- Drop RequestAccessPolicy's in-policy profile selector. Use normal configuration rollout, or
  per-entry `conditions` keyed on ConfigData, for staged switching. Keep the ConfigData URL
  allow-list import as a condition source if it is needed.
Pre-release status means no compatibility shim is required.

## Notes
- `skills/04-review/request-restriction/regression-guards/mode-is-required-no-default-polarity.md`
  explicitly lists "real configurations repeatedly need allow **and** deny simultaneously" as its
  re-evaluation trigger, with `denyRuleGroups` + `allowRuleGroups` + `defaultAction` as the shape to
  adopt. RequestAccessPolicy, which arrived later, still does not offer that shape. Its existence is
  evidence the trigger has fired.
- `skills/04-review/plugin-metadata/superseded/unified-condition-layer.md` unified the
  *predicates*. This finding concerns the duplicated *policy layer* above them.
- Related over-design findings: `gw-plugin-05` (two CORS engines) and `gw-plugin-06` (upstream-steering
  sprawl).

## Related findings

- gw-plugin-05, gw-plugin-06.

## Verification (independent)

- **Verdict**: CONFIRMED (severity P3 unchanged; direction item)
- `edgion-resources/.../plugin_configs/request_access_policy.rs:42-187`: profiles → `required_rule_groups` → `any_of_rules`, with no deny list or `defaultAction`; `request_restriction.rs:107-119` uses a single `mode` + `ConditionSet`. Both diverge from the canonical allow/deny + `defaultAction` shape (CLAUDE.md rule 11).
- `skills/04-review/request-restriction/regression-guards/mode-is-required-no-default-polarity.md` names "need allow and deny simultaneously" as its re-evaluation trigger, which supports filing this as a direction item. No incorrect behavior is claimed, so P3 is right.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P3 unchanged)
- `ip_restriction`, `request_restriction`, `request_access_policy` plugins and their `plugin_configs` have no diff.
