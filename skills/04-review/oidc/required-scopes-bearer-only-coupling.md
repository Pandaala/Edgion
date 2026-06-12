---
name: required-scopes-bearer-only-coupling
description: Use when reviewing OIDC requiredScopes findings claiming scope enforcement is "silently bypassed" / "an authorization bypass" when bearerOnly=false; documents that the bearerOnly coupling is intentional and locked by a unit test, and that the genuine defect (now fixed) was the user docs not stating the bearerOnly=true prerequisite.
status: accepted-tradeoff
finding_id: fr-oidc-01
closed: 2026-05-30
---

# OIDC `requiredScopes` Only Enforced When `bearerOnly=true` — ACCEPTED-DESIGN (doc gap fixed, guard kept)

## Conclusion

`validate_required_scopes` (`edgion-gateway/src/plugins/http/openid_connect/config_validation.rs:88-91`) returns `Ok(())` immediately when `!self.config.bearer_only`, so `requiredScopes` is enforced **only in bearer-only mode**. This coupling is an intentional, reviewed design decision — not a silent code defect or an exploitable authorization bypass. The real, confirmable problem was a **documentation gap**: the user-guide parameter tables described `requiredScopes` as unconditional with no `bearerOnly=true` prerequisite. That gap is now fixed (en + zh-CN docs carry a note). The guard stays.

Fix suggestions of the form "remove the `bearer_only` guard so scope enforcement applies in all modes", "add a config-validation error/warning when `requiredScopes` is non-empty with `bearerOnly=false`", or re-reports framing this as a "silent authorization bypass" are **not accepted**.

## Core Rationale

**1. The coupling is intentional and locked, not an accidental gap**

`docs/review/openid-connect-review-card.md:23` explicitly checks off "Scope: only effective when `bearerOnly=true`". The behavior is pinned by a dedicated unit test `test_validate_required_scopes_only_applies_to_bearer_only_mode` (`tests.rs`), which asserts the early-return for `bearerOnly=false` and the 403 enforcement for `bearerOnly=true`. Removing the guard would silently invert a documented decision and break a passing test.

**2. It is not an exploitable bypass — it requires operator misconfiguration driven by incomplete docs**

All three verification paths (introspection cached `token_verify.rs:25`, introspection live `:74`, JWKS `:169`) delegate unconditionally to `validate_required_scopes`, so session-mode (`unauthAction: Auth`, `bearerOnly=false`) users do pass regardless of scopes. But that only causes a real-world authorization gap if an operator was *led to believe* `requiredScopes` would enforce in session mode. The harm vector is the docs, not the code path — so the fix belongs in the docs (and optionally config validation), not in flipping runtime behavior.

**3. The genuine defect — public doc + missing prerequisite — is fixed**

`docs/en/user-guide/edgion-plugins/openid-connect.md` and its `docs/zh-CN/...` mirror previously described `requiredScopes` as "Required scopes; token missing any will be rejected" with no `bearerOnly=true` prerequisite. Both now carry an explicit note that scope enforcement applies only in bearer-only mode and that in the default session/login mode `requiredScopes` is accepted at config time but not enforced at runtime, with guidance to set `bearerOnly: true` to restrict access by scope.

## Fix Suggestions Not Accepted

- Remove the `bearer_only` guard from `validate_required_scopes` — silently changes documented, test-locked behavior; would require inverting a passing unit test; risk far outweighs the benefit of a documented, doc-fixable inconsistency.
- Add a config-validation hard error in `get_validation_error` when `requiredScopes` is non-empty with `bearerOnly=false` — considered and declined for the doc-only close; it would turn previously-accepted configs into startup errors (potential breakage for existing deployments that set both and rely on the documented no-op). Revisit only via a deliberate brainstormed task if the doc note proves insufficient.
- Re-report framing it as a "silent gap / invisible / authorization bypass" — the design intent is documented internally (review card + unit test); the user-facing gap is the doc, which is now closed.

## Re-evaluation Triggers

Re-open this decision only if:

- The team decides session-mode scope enforcement is a required feature (a deliberate product change, handled as a new task — not a review fix), in which case the guard, the review card, and the unit test all change together.
- The user-guide note added here is removed or drifts out of sync with the `bearerOnly`-only enforcement semantics.
- A new verification path is added that enforces (or should enforce) `requiredScopes` independently of `validate_required_scopes`.

## Reference Cases

- fr-oidc-01 (closed 2026-05-30, doc-only) — `tasks/bug/fr-oidc-01-required-scopes-bypassed-in-session-mode.md` (Opus verification: ADJUST, downgrade P2 → P3, Doc/UX hardening).
- Source: `edgion-gateway/src/plugins/http/openid_connect/config_validation.rs:88-133`
- Internal design lock: `docs/review/openid-connect-review-card.md:23`; unit test `test_validate_required_scopes_only_applies_to_bearer_only_mode`.
- Docs fixed: `docs/en/user-guide/edgion-plugins/openid-connect.md` + `docs/zh-CN/user-guide/edgion-plugins/openid-connect.md` (`requiredScopes` row + note after the Basic Parameters table).
