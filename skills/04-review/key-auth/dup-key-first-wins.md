---
name: dup-key-first-wins
description: Use when reviewing KeyAuth resolve_credential changes; documents that cross-group duplicate raw keys must use first-wins + warn (not fail-fast, not last-wins).
---

# KeyAuth — cross-group duplicate keys are first-wins

The KeyAuth `secret_groups` feature (pilot for the plugin-grouping series)
defines `secret_groups` as a management convenience, NOT a tenant isolation
boundary. When the same raw API key appears in two groups, the controller
must:

1. **Keep the first occurrence** (iteration order of `secret_groups`),
2. **Drop the later occurrence**,
3. **Emit a `tracing::warn!`** that carries both `existing_group` and
   `incoming_group` fields so operators can locate both Secrets.

Implementation lives in:
`src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/key_auth.rs`

When reviewing changes in this file, reject:

- **fail-fast** (returning an error / rejecting the EdgionPlugins resource):
  one bad partner Secret would take down the whole plugin, which violates
  the "management convenience" framing.
- **last-wins**: ordering is HashMap-iteration-dependent and unpredictable
  across runs, which makes incident analysis harder.
- **Silent drop**: the warn line is the only audit trail; do not remove it.

Same convention applies to future grouping plugins (HmacAuth, JwtAuth,
BasicAuth) — see `tasks/todo/plugin-grouping/01-key-auth-spec.md` §3.
