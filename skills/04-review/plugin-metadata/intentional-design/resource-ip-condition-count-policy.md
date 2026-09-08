---
name: resource-ip-condition-count-policy
description: Use when reviewing EdgionPlugins aggregate IP-condition limits and their distinction from URL allow-list consumer limits.
knowledge_type: intentional-design
---

# EdgionPlugins has no aggregate IP-condition quota

## Conclusion and scope

The user explicitly chose to remove the resource-wide 10,000 IP-condition cap
on 2026-09-08. An otherwise valid `EdgionPlugins` resource is not invalid merely
because its entry, body, dye, RequestRestriction, or RequestAccessPolicy
conditions collectively exceed that count, including after ConfigData expansion.

This decision removes the aggregate validation diagnostic in the shared schema
and Gateway compiler, plus the Controller's post-expansion warning. It does not
change per-condition validity, unresolved-reference handling, plugin policy,
the canonical reject gate, or last-known-good publication rules.

## Rationale and evidence

D0081 correctly identified an inconsistency: `validate_plugins` reported the
aggregate cap violation, while `compile_plugins` only appended a diagnostic and
never recorded it in `PluginRuntime::invalid_plugins`. The resource could remain
activatable despite the Controller reporting `Accepted=False`.

The original proposal was to add fail-closed enforcement. During review, the user
questioned the cap's value and explicitly requested its removal instead. This is
a changed product constraint, not a finding that the former implementation was
correct. The former counter measured IP condition nodes, not expanded CIDRs,
compiled bytes, or total execution cost; no capacity evidence for its 10,000
threshold was established in this investigation.

Sources:

- `edgion-resources/src/resources/edgion_plugins/validate.rs`: shared validation
  continues checking individual entries without resource-wide IP counting.
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/condition_tree.rs`:
  ConfigData expansion still precedes credential resolution; it no longer adds
  aggregate-cap warnings to `RequestAccessPolicyConfig.resolution_warnings`.
- `edgion-gateway/src/plugins/http/plugin_store.rs`: compilation and publication
  use the existing runtime validity and update policies without the cap diagnostic.

## Alternatives and limits

Adding a leading HTTP 500 gate for this count was rejected in favor of removing
the constraint. The benefit is consistent acceptance of large valid plugin
configurations without an unsupported numerical cutoff. The accepted cost is
that this aggregate counter no longer bounds matcher count or worst-case IP
condition evaluation work. This is not a claim of unlimited practical capacity.

The following independent limits remain supported and enforced:

- `RequestAccessUrlAllowListData::validate` limits one URL allow-list data object.
- `UrlAllowListRefValidation.max_ip_rules` limits data imported at a consumer binding.
- `MAX_REFERENCED_IP_RULES`, `ConditionSet::ip_match_count`, and
  `UrlAccessCandidate::ip_rule_count` remain for those consumers.

Individual malformed conditions still use the publication enforcement documented
in [condition validation](../regression-guards/conditions-timerange-preparse-validation.md).
The shared matcher and condition ownership remain governed by
[ConditionSet ownership](../mandatory-design/condition-set-single-owner.md).

## Verification and re-evaluation

Focused regressions cover schema/Gateway acceptance at 10,000 and 10,001 nodes,
delta and full-snapshot publication, continued rejection of malformed conditions,
and Controller expansion beyond 10,000 nodes without aggregate warnings.
Fresh verification for this change:

- `cargo test -p edgion-gateway --lib plugins::http::plugin_store::compile_plugins_tests`: 20 passed.
- `cargo test -p edgion-controller --lib condition_tree_`: 8 passed after correcting
  the new test's `PluginConditions` import.
- `cargo test -p edgion-resources --lib request_access`: 23 passed, including
  `url_ref_validation_limits_enforce_hard_bounds`.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` completed successfully. Clippy reported
  existing workspace warnings; none pointed to added or modified code lines.
- Agent-doc consistency, SSA force, metrics inventory, and Pingora source policy
  guards passed. Independent read-only review returned LGTM.

Existing integration-test files are unchanged by this task; the focused tests
exercise the affected compilation/publication seams. No full integration or
workspace-wide test suite was run.

Re-evaluate if measurements establish a concrete configuration-build memory or
request-evaluation budget, or if multi-tenant configuration admission needs a
quota. Any replacement must define its units, scope, and update behavior; do not
restore the former constant as an assumed matcher capacity limit.

## Resolution references

- D0081 / `fail-closed-plugin-ip-matcher-limit-violations`: originally a real
  plugin-publication inconsistency; resolved by the user-authorized removal of
  the aggregate constraint, not by adding a reject gate.
