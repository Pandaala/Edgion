---
name: waf-production-profile-acceptance
description: Verify selected native WAF rules through strict compilation and Gateway behavior.
knowledge_type: regression-guards
---

# Selected production WAF rules need strict and wire acceptance

## Conclusion and scope

The selected WAF acceptance profile is the three repository-tracked
`EdgionPlugins/Waf/DynamicTest` YAML configurations listed in
`edgion-waf/acceptance/production-v1.json`. This is a narrow initial production
profile, not full CRS support. The profile uses the production
`WafConfig::compile_generation` path, verifies exact source digests and rule
IDs, and exercises allow, deny, response denial, and detection-only decisions.
Missing or changed fixtures and shrinking case coverage fail the dedicated gate.

## Trigger and root cause

The exploratory `ftw_inventory` test can return early without external corpus
configuration. Its executable-subset compiler intentionally omits unsupported
rules, and its offline transactions run in detection-only mode. A pass there
cannot establish that the same rules strictly compile or enforce in the
Gateway. The dedicated gate keeps these evidentiary scopes separate.

## Verification and limits

`cicd/checks/run_waf_production_acceptance.sh` emits a JSON case report, runs
the Gateway generation pinning test, and uses the existing shared integration
topology for the `WafDynamic` wire and hot-reload scenario. The integration
report must contain a passing `Dyn_WafDynamic` entry. The dedicated workflow
uploads both reports. The selected profile begins with nine cases and zero
exclusions; the FTW corpus and external engines are not required by this gate.
Future expansion must review case intent, fixture provenance, exclusions, and
Gateway behavior rather than infer compatibility from a percentage alone.

Local verification on 2026-09-23 passed strict compilation for all three
configurations, all nine selected offline cases, the Gateway in-flight
generation test, and the targeted `Gateway/WafDynamic` integration scenario.
The integration report recorded `Status: PASS`, `Dyn_WafDynamic` PASS, and zero
failed gates. Workspace check, Clippy, formatting, agent-doc validation, SSA,
metrics inventory, Pingora source policy, and Gateway API transform checks
also passed in this run. These results apply to this source and test setup,
not to the larger FTW/CRS corpus.

Re-evaluate when selected rules, WAF compilation, body handling, publication
policy, or integration scenario registration changes. Source ID:
`waf-production-rule-acceptance`.
