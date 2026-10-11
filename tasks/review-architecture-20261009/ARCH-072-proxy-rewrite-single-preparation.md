---
id: ARCH-072
status: candidate
kind: architecture-debt
priority: P3
partition: PART-212
analyst: /root/analyze_outbound
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Retain ProxyRewrite compilation results from one cold preparation pass

ProxyRewriteConfig::precompile in edgion-resources/src/resources/edgion_plugins/plugin_configs/proxy_rewrite.rs builds the configured regex and JSON Patch caches, then calls check_fields, which builds both again and discards its outputs. A successful mixed configuration performs two Regex::new calls and two JsonBodyRewrite::compile calls. The second JSON pass repeats operation serialization/plan-size checks, owned pointer parsing, literal profile cloning and template compilation. This is duplication within one preparation invocation.

The sole production precompile caller is Gateway ProxyRewrite::create, which clones the configuration then prepares it; other callers are tests. A valid regex ^/old$ with replacement /new and one add operation containing a 4096-byte literal constructs two compiled pairs, retains the first pair and drops the second literal/pointer/template plan. This is a source-derived call/allocation count, not measured throughput, request-path overhead or a security incident.

Use one private cold preparation routine which checks the Host conflict and compiles regex/JSON outputs, returning a private tuple of compiled results. Stateless check_fields calls that routine and extracts/discards the result at its independent validation boundary. precompile clears both caches, invokes the routine once, and publishes the pair only after complete success. No public prepared API, framework, memoization, serialized state or global cache is required.

Preserve check_fields diagnostic priority and wording: Host conflict before regex before JSON Patch. precompile currently does regex/JSON/Host work, but exposes only a boolean and cache outcomes; changing that internal order need not change observable diagnostics. Preserve clearing before preparation, no partial or stale cache on failure, and absent configuration producing absent caches. Do not merge independent Controller and Gateway admission gates. ARCH-050 separately owns repeated invocation-level validation orchestration.

Preserve the accepted regex envelope and current engine acceptance tradeoff, URI/header semantics, uri precedence over regexUri, and the separate runtime URI/argument/capture substitutions. Preserve all six JSON Patch operations in order, null versus absent values, exact operation source arity, template validation and SecretRef rejection, named scalar sources, root/pointer escaping, input/output/depth/node/work limits and schema-hidden serde-skipped caches. Canonical request BodyStorage and streaming-response ownership do not change. ARCH-037 separately owns runtime token boundaries.

Existing Resources tests cover valid/invalid regex, Host conflict, stateless validation, all operations, null/arity, templates, pointers, plan complexity and cache serialization. The templates_compile_once test checks the resulting cache length and acceptance, rather than the number of preparation passes. Gateway tests inspect construction, publication and bounded mutation failures. Future verification should retain mixed-error diagnostic priority, valid-to-invalid cache clearing, empty/absent results and real compiler semantics. Source inspection should show one preparation call within precompile; tests should retain private access rather than widen visibility or allocate for mocks.

A cheap Host check after the original compilation would also remove the extra calls, but leaves two validation orchestrators that can drift as JSON/regex policy changes. The private per-configuration preparation owner keeps the correction bounded to the schema owner and its tests; no Gateway runtime rewrite or generic plugin compilation framework is needed.

Original plugin-config-regex-dfa-size-limit and request-template-policy-unification decisions were read and retain their scope. ARCH-050 and ARCH-037 have different root causes. Expected benefit: one unused regex and JSON plan per successful precompile is removed, with one owner of preparation validation. Static read-only review only; no fresh builds, tests, benchmark, application or network evidence. Primary baseline SHA256: 7fce439dbfcde54c6739c23cf2cad3a1773e5ab9c21a32372188c85b1f9ee02f. PART-212 records original and actual caller/test evidence; independent verification is pending.
