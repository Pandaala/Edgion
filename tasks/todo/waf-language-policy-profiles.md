# Language-specific WAF policy profiles

Status: Audited examples implemented; deployment/performance validation open; P2

Provide operator-selected Java, PHP, and mixed-stack profiles through the existing WafPolicy, WafRuleBundle, digest, and Selector mechanisms. Do not infer the application language from untrusted request data.

- Audit rule, chain, TX, marker, and scoring dependencies before omitting a language family; keep generic attack coverage.
- Document the exact omitted protections. Unknown or mixed applications keep the default protection set.
- Preserve the validated default fallback for absent, disabled, or unknown Selector choices and strict validation of operator configuration.
- Verify Controller/Gateway parity, hot reload, invalid-generation retention, and in-flight generation ownership.

Start from `skills/04-review/waf/regression-guards/named-profile-selection-and-bundle-publication.md` and `config/waf/crs-4.25.1-supported-v1/`.

The independently reviewed prerequisite design is [WAF tag selection](../pending/waf-tag-selection/README.md).
Audited Java/PHP examples now live under the pinned supported package's
`language-selection-examples/`, with exact omissions and PL1/PL4 findings/score
acceptance in `edgion-resources/tests/waf_tag_selection.rs`. Generic/mixed defaults
remain unfiltered. See the [verification record](../pending/waf-tag-selection/05-verification.md).
Task closure remains open for live/Kubernetes reload and comparative performance
validation; source/fixture presence alone does not complete those gates.
