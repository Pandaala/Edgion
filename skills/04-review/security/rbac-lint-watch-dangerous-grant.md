---
name: rbac-lint-watch-dangerous-grant
description: Use when reviewing lint_rbac_policy / RBAC dangerous-grant findings claiming "watch" on Secret or kinds:["*"] passes lint with no advisory warning.
---

# `lint_rbac_policy` dangerous-grant check covers `watch` — FIXED (m1-01, 2026-05-29)

## Conclusion

`lint_rbac_policy` (`edgion-controller/src/authz/lint.rs`) now treats `watch` as a
read verb for the advisory dangerous-grant WARNs. A `watch Secret` or `watch *`
grant trips the same warning as `get`/`list`/`list-keys`. Re-reports of the form
"`has_read_verb` omits `watch`, so a `watch Secret` / `watch *` policy passes
lint silently" are **not accepted** — verify the `matches!` arm still includes
`"watch"` before re-reporting.

## Core Rationale

**1. Why `watch` belongs in the read-verb set**

`watch` is a valid `Verb` (`authz/mod.rs`) and the fed-client watch path
(`fed_client/mod.rs`) goes through the same `authorize()` gate the HTTP admin API
uses. A `watch`-allowed policy is therefore enforced for real — it streams the
same credential data a `get`/`list` would expose — so the lint must flag it with
equal weight. The pre-fix set (`get | list | list-keys`) was a false negative.

**2. The advisory is warn-only; enforcement was always fail-closed**

This is a missing *advisory WARN*, not a missing enforcement check. The
enforcement path is fail-closed (an unmatched verb/kind grants nothing) and was
never affected. The fix only widens which grants the operator is warned about; it
touches no `authorize()` decision.

**3. Real data exposure is doubly gated and remains documented**

Even with the warning fixed, actual Secret streaming requires two non-default
operator actions: writing the `watch Secret`/`watch *` policy *and* removing
`Secret` from `conf_sync.no_sync_kinds` (default keeps `Secret` in
`DEFAULT_NO_SYNC_KINDS`, so a watch resolves to an "Unknown kind" error frame,
not a credential stream). The operator-facing exposure note in
`docs/en/ops-guide/federation-rbac.md` ("Wildcard read exposes credential
material") was updated to list `watch` alongside `get`/`list`/`list-keys`.

## Fix Suggestions Not Accepted

- "Make the lint fatal / reject `watch Secret` at load time" — not accepted; the
  lint is advisory by design (`lint.rs` module doc), enforcement is the
  fail-closed boundary, and a hard reject would break the `no_sync_kinds`-gated
  legitimate-watch case.
- "Drop the Secret/`*` dangerous-grant WARNs entirely since enforcement is
  fail-closed" — not accepted; the WARNs exist to surface operator
  over-grants before they combine with a `no_sync_kinds` override.

## Re-evaluation Triggers

Re-open this decision only if:

- A new read-class verb is added to `Verb` (it must be added to the
  `has_read_verb` `matches!` arm at the same time).
- `Secret` is removed from `DEFAULT_NO_SYNC_KINDS`, which would make the watch
  exposure path default-on and warrant escalating the lint from advisory.

## Reference Cases

- m1-01 (2026-05-29) — original finding; P3 missing advisory warning.
- `edgion-controller/src/authz/lint.rs` — `lint_rbac_policy`, dangerous-grant block
  + `watch_secret_emits_dangerous_grant_warn` / `watch_wildcard_emits_dangerous_grant_warn` regression tests.
- `docs/en/ops-guide/federation-rbac.md` — "Wildcard read exposes credential material" security note.
