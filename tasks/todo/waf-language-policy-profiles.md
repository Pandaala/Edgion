# Language-specific WAF policy profiles

Status: Planned; P2

Provide operator-selected Java, PHP, and mixed-stack profiles through the existing WafPolicy, WafRuleBundle, digest, and Selector mechanisms. Do not infer the application language from untrusted request data.

- Audit rule, chain, TX, marker, and scoring dependencies before omitting a language family; keep generic attack coverage.
- Document the exact omitted protections. Unknown or mixed applications keep the default protection set.
- Preserve the validated default fallback for absent, disabled, or unknown Selector choices and strict validation of operator configuration.
- Verify Controller/Gateway parity, hot reload, invalid-generation retention, and in-flight generation ownership.

Start from `skills/04-review/waf/regression-guards/named-profile-selection-and-bundle-publication.md` and `config/waf/crs-4.25.1-supported-v1/`.
