---
id: ARCH-067
status: candidate
kind: architecture-debt
priority: P3
partition: PART-210
analyst: /root/analyze_outbound
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Borrow CtxSet transform fields during configuration validation

`TransformConfig::get_transform_type` constructs an owned `TransformType`, but its only callers are four pattern checks inside `CtxSetConfig::check_fields` and `CtxSetConfig::validate`. The Gateway does not consume this projection: `CtxSet::apply_transform` reads `replace.as_ref()`, the copied substring tuple, `case.as_ref()`, and `prefix`/`suffix.as_deref()` directly. The projection therefore duplicates operator strings only to decide which validation applies.

After the existing multiple-operation check, each validator calls the helper once to check `Replace` and again to check `Substring`. A valid prefix or suffix is cloned on both calls and immediately discarded. A replace operation clones its pattern and replacement twice, although validation never reads the replacement. For example, one valid rule with `name: key`, `value: v`, and a 4,096-byte ASCII prefix copies 8,192 prefix bytes per execution of either validation path. This is source-derived copied volume, not a measured allocation benchmark or request-path performance claim.

This occurs in real cold configuration paths. `EdgionPlugin::validation_error` dispatches CtxSet to the shared schema validator; `EdgionPlugins` validation is used by Controller preparse and Gateway compilation. Gateway `CtxSet::create` separately clones the configuration it legitimately owns and calls `validate` to populate its compiled regex/template caches. Those owned runtime fields remain necessary. A workspace-wide Rust call-site search finds no consumer of `get_transform_type` or `TransformType` outside the four validation calls; their remaining appearances are definitions and re-exports from `plugin_configs/mod.rs` and `edgion_plugins/mod.rs`.

Replace the two owned projections in each validator with direct borrowed inspection of `transform.replace.as_ref()` and the copyable `transform.substring` tuple, after the existing operation-count gate. Compile the borrowed replacement pattern and preserve the existing substring bound check. Remove the unused helper, enum, and its two re-exports after repeating the call-site audit. No new borrowed enum, validation framework, shared runtime registry, or prepared configuration cache is needed.

Preserve all current validation semantics: zero or one operation; `trim: false` does not count; multiple-operation diagnostics and their order; extraction validation before transform validation and template validation afterward; regex/template cache clearing and population in `validate`; and the accepted sequential cross-rule context/template behavior. Keep the Gateway's defensive first-operation precedence for malformed configurations that bypass validation, its Unicode-scalar substring behavior, and its missing-compiled-regex fallback. This change does not merge the stateless and cache-populating validators or alter request-time output ownership.

Relevant original authority is the reuse-first/canonical-owner guidance and the CtxSet one-transform-operation guard. That guard requires matching `check_fields`/`validate` rejection and defensive runtime behavior, not this owned projection. Existing resource tests cover operation acceptance, matching multiple-operation errors, invalid regex/substrings, and chained rules; Gateway tests cover defensive precedence, substring semantics, and missing regex fallback. Verification should retain those cases, compare both validators on representative prefix/suffix/replace configurations, and establish structurally that validation borrows the configured strings. No benchmark or mock-specific ownership change is necessary.

Keeping the public projection would retain an otherwise unused API and the copying that prompted the finding. Introducing a borrowed projection adds an abstraction with no external consumer. Direct field access is the bounded alternative. The finding is an architecture/maintenance optimization of cold validation, priority P3; it is not a security defect or an admission-parity claim. It is distinct from ARCH-050's repeated invocation work and ARCH-062's path-match projection. No external downstream API consumers were audited, and no builds or tests were run in this read-only review.
