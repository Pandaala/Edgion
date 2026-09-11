---
name: ctx-provenance-unmasking-risk
description: Review the accepted configuration-dependent risk of sensitive values copied into explicitly unmasked ctx variables.
knowledge_type: intentional-design
---

# Sensitive ctx aliases: accepted configuration-dependent logging risk

## Conclusion and scope

On 2026-09-11 the user explicitly accepted the configuration-dependent risk in
`ctx-sensitive-provenance-unmasking` (campaign candidate `D0044`) and chose no
runtime change. The defect remains real: copying a credential into an ordinary
ctx name loses its sensitive source, and an explicit log declaration plus a
platform unmasking permission for that name can expose the value even in
`access_log_sensitive.mode: replace`. This is accepted risk, not a fix, false
positive, or a claim that provenance enforcement is impossible.

The acceptance concerns existing configurable ctx value propagation. It does
not relax the [design rules](../../../00-design-rules.md#a-sensitive-data--log-redaction)
for direct credential projections or newly introduced canonical secret-bearing
ctx keys. Fixed header/claims blocklists and explicit log declarations remain.

## Rationale and evidence

- `edgion-gateway/src/plugins/http/ctx_set/plugin.rs`: `process_rule` reads
  `KeyGet` or the shared template result as a `String`; `run_request` writes it
  with `PluginSession::set_ctx_var` under the configured `CtxVarRule.name`.
- `edgion-gateway/src/plugins/runtime/session_adapter.rs`: `key_get` and
  `get_ctx_var` return `Option<String>`. The shared template engine at
  `edgion-gateway/src/link_sys/template/engine.rs` also returns a plain string.
- `edgion-gateway/src/ctx.rs`: `PluginAuxState.ctx_map` is a
  `HashMap<String, String>`; `set_ctx_var` retains no source classification.
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/access_log_extern.rs`:
  `is_blocked` classifies the declared source/name. Gateway `compile_plugins`
  drops invalid declarations, but cannot infer the provenance of an alias.
- `edgion-gateway/src/observe/access_log/log_ext.rs`: `resolve_field` reads the
  ctx string; `should_mask` tests the declared source and `name` against
  `UnmaskedKeys`. The output `key` does not determine the unmasking permission.

For example, a platform permits `unmaskedKeys.ctx: [tenant_id]`, a route's
CtxSet copies `Authorization` to `tenant_id`, and AccessLogExtern declares
`{key: tenant, from: ctx, name: tenant_id}`. A short bearer token then appears
verbatim as `ext.tenant`. A value-length cap is not credential protection.

The existing [claims projection guard](../../ctx-set/regression-guards/claims-access-log-filter.md)
still protects direct declarations of `jwt_claims`, `oidc_claims`, and
`jwe_payload`; it does not prove aliases safe. Precompiled templates prevent
recursive template injection, not this loss of provenance.

## Alternatives and limits

The user judged this configuration combination unlikely and declined the cost
of a shared provenance model across extraction, templates, ctx storage, and log
rendering. No deployment-frequency measurement was performed. Comprehensive
tracking is possible, but requires propagation/overwrite/transform semantics
and a broader audit of writers, including DSL. A CtxSet-only guard would have
limited coverage; conservative propagation can hide legitimate business values
extracted from sensitive inputs.

Ordinary request data alone cannot create the required plugin and log-policy
configuration. The ctx unmasking list is empty by default. Under `replace`, an
unlisted ctx value is masked; without a log declaration it is not collected.
The documented `off`/`wrap` modes can retain raw values without unmasking and
are not covered by that default-mode protection. A named cookie is not
categorically forbidden merely because the complete Cookie header is blocked;
cookie projection has its own per-name policy.

Review ctx unmasking permissions together with configurations allowed to write
those variables. This is an operational mitigation, not an enforced security
boundary: a route author who can change CtxSet can redirect sensitive data into
an already permitted name without changing the platform's EGC policy. The
accepted cost is credential exposure in logs and downstream copies when the
configuration conditions hold.

## Verification and re-evaluation

The call chain and existing masking/blocklist test sources were inspected on
2026-09-11. No runtime fix or new regression test was introduced. User-visible
behavior, schema, and defaults are unchanged, so user documentation and
integration scenarios do not require changes. Historical results in the claims
guard and B0071 are not fresh verification of this acceptance.

Closure verification on 2026-09-11: independent read-only review returned LGTM
after correcting the claims guard's existing SHOULD/MUST mismatch for new
canonical secret-bearing keys. `cargo fmt --all`,
`cargo check --workspace --all-targets`, and
`cargo clippy --workspace --all-targets` completed successfully. Clippy reported
existing warnings in unchanged Rust files; this was not a warning-free run.
The `cicd/checks/` scripts `validate_agent_docs.py`, `check_ssa_force.py`,
`check_metrics_inventory.py`, and `check_pingora_source_policy.py` also passed.
No full test suite was run for this no-code decision.

Reconsider if untrusted tenants can author plugins while relying on the platform
ctx unmasking list as a confidentiality boundary; if actual misconfiguration or
credential leakage is observed; if ctx unmasking becomes common or enabled by
default; or if a shared provenance mechanism makes comprehensive propagation
substantially cheaper. Revalidate changed writers and sinks before applying this
decision to a new report. General sensitive-log rules remain authoritative
outside this explicitly accepted existing gap.

## Resolution references

- `ctx-sensitive-provenance-unmasking` / `D0044`: real defect, risk explicitly
  accepted by the user on 2026-09-11; no code change requested. Origin:
  [B0071](../../../../tasks/pending/edgion-gateway-file-optimization/batches/B0071-ctx-set-bounded-transform-execution.md).
