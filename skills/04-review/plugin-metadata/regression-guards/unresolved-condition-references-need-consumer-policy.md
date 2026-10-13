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

An unavailable required `ipRefs`, `secretMatch.secretRef`, or HMAC `secretGroups[].secretRefs` dependency is not an ordinary false predicate. It never matches, `negate` never inverts it, and the owning consumer decides whether the incomplete candidate is unavailable or the whole runtime must fail closed.

Fix suggestions that collapse unresolved dependencies back into a boolean or add one global allow/deny switch are **not accepted**.

## Core Rationale

**1. Boolean false has opposite security effects across consumers**

`RequestRestriction` `Deny` and entry-level `run` can admit traffic when a required dependency becomes false. `RequestRestriction` `Allow` and `RequestAccessPolicy` instead deny safely when an incomplete allow candidate cannot match. Applying `negate` to the unresolved state reverses either conclusion and is therefore forbidden.

**2. Static construction validity and live IP availability are separate**

The Controller resolves credential payloads and recomputes IP read scopes from actual owning fields. It never expands IP/CIDR strings into consumers. The Gateway binds original effective condition nodes before cloning, then uses one adapter-owned ConfigData snapshot for whole-runtime preflight and subsequent scalar membership/Selector reads. Missing same-namespace lists can recover without recompiling the consumer. Cross-namespace slots require a positive Grant proof or current Cluster visibility; an absent negative marker is not permission. Source, target, root/leaf/slot and scope consistency are checked before cache reuse.

**3. Enforcement follows the consumer boundary**

Static malformed inputs, owner-binding failures and unresolved credentials keep the existing cold fail-closed contract. Live required IP dependencies cover all active entry/body roots and RequestRestriction Deny roots, before outer skip/run, early RequestId, backend or nested ExtensionRef dispatch. Their temporary absence returns the generic 500; it does not permanently seal the runtime. Allow restrictions skip incomplete alternatives, Dye remains observational, and RAP optional IP omissions form a successful empty union. Before headers, response stages use existing local-reply arbitration. After commitment, body failure discards both current and queued output, preempts/releases every body and trailer owner, clears conversion output/budgets and returns a non-retryable transport error without rewriting committed status. Body success adds no chunk log or allocation. Controller status/requeue and the existing canonical diagnostic remain the observability owners.

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

## RAP IP resolution distinction

RAP IP references remain partial-good. Missing, disabled, wrong-type and denied optional targets contribute no matcher; usable siblings remain. An all-missing union is successful and empty: positive primary membership is false, while secondary/URL shared IP Condition negation may invert false. Invalid input or binding is unresolved and cannot invert. Primary RAP `negate` was previously inert despite its schema exposure; ARCH-028 removes and rejects its presence rather than silently introducing complement behavior. Supplemental negation remains ANDed with positive primary membership.

Historical cl-01 used Controller-expanded IP payloads to distinguish incomplete resolution; ARCH-028 replaces that mechanism with live snapshot availability and positive per-slot read policy. It preserves the credential contract and consumer polarity, not the removed payload representation. ReferenceGrant revocation retains the existing Controller requeue window; current Cluster-only visibility is checked immediately in the same data snapshot. No cross-store transaction or per-request Grant lookup is claimed.
