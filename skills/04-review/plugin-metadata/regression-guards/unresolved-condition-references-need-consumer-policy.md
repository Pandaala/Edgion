---
name: unresolved-condition-references-need-consumer-policy
description: Use when reviewing unresolved ipRefs, SecretMatch, or HMAC Condition behavior; unresolved dependencies are distinct from ordinary non-matches, cannot be negated, and follow consumer-specific fail-closed policy.
status: fixed
finding_id: cl-01
closed: 2026-08-24
knowledge_type: regression-guards
---

# Unresolved Condition References Need Consumer Policy — FIXED

## Conclusion

An unresolved `ipRefs`, `secretMatch.secretRef`, or HMAC `secretGroups[].secretRefs` dependency is not an ordinary false predicate. It never matches, `negate` never inverts it, and the owning consumer decides whether the incomplete candidate is unavailable or the whole runtime must fail closed.

Fix suggestions that collapse unresolved dependencies back into a boolean or add one global allow/deny switch are **not accepted**.

## Core Rationale

**1. Boolean false has opposite security effects across consumers**

`RequestRestriction` `Deny` and entry-level `run` can admit traffic when a required dependency becomes false. `RequestRestriction` `Allow` and `RequestAccessPolicy` instead deny safely when an incomplete allow candidate cannot match. Applying `negate` to the unresolved state reverses either conclusion and is therefore forbidden.

**2. Resolution completeness is available before request evaluation**

The Controller writes `resolvedIps`, `resolvedValues`, and `resolvedCredentials` into the conf_sync payload. The Gateway distinguishes absent Controller-owned IP payload from a successfully resolved empty IP union. A Secret with no usable string value remains unresolved, as do configured HMAC groups with no resolved credentials; an empty verifier cannot represent a usable credential source.

**3. Enforcement follows the consumer boundary**

Active entry/body `PluginConditions` mark the compiled EdgionPlugins runtime non-activatable and reuse `seal_fail_closed`. RequestRestriction `Deny` uses its existing generic 500 configuration-error path; `Allow` skips the incomplete group. RequestAccessPolicy skips the incomplete allow candidate. Dye conditions remain observational and simply do not match. Missing or denied references retain the Controller status and `edgion_controller_resource_abnormal` signal. `secretMatch` also reports a present Secret whose selected `secretKey` is missing, non-UTF-8, or blank; other present-but-unusable Secret consumers may still rely on their resolver-specific status or warning plus Gateway enforcement. No path adds per-request tracing or a duplicate metric.

## Fix Suggestions Not Accepted

- Compile an empty matcher for a missing reference — this erases the difference between successful empty content and a dependency failure.
- Put resolved-payload requirements in shared admission validation — admission sees operator YAML before the Controller populates runtime fields.
- Add a global observe-only or fail-open switch — the same fallback has opposite effects in allow lists, deny lists, `run`, `skip`, and negated predicates; an emergency override requires a separate product design.
- Change IpRestriction `allowRefs` / `denyRefs` behavior in this fix — that plugin has its own established reference policy and was outside cl-01.

## Re-evaluation Triggers

Re-open this decision only if:

- Condition evaluation gains a new consumer whose safe unresolved behavior cannot use either fail-closed runtime sealing or unavailable-candidate semantics.
- A separately designed emergency policy defines bounded startup-only override behavior, observability, and every consumer polarity.

## Reference Cases

- cl-01 (2026-08-24) — unresolved Condition references no longer evaluate as negatable false.
- `edgion-gateway/src/plugins/runtime/conditions/evaluator.rs`
- `docs/en/user-guide/http-route/filters/edgion-plugins/conditions.md`
- [Unified Condition Layer](../superseded/unified-condition-layer.md)

## Resolution references

- `cl-01` — 2026-08-24.
