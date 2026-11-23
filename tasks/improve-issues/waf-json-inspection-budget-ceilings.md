# Decide ceilings for explicit WAF JSON inspection budgets

Open question transferred from the additional evidence in `waf-02`. An
`EdgionPlugins` author can set JSON lexical, structure and result budgets
above engine defaults. In `edgion-resources/src/resources/edgion_plugins/plugin_configs/waf.rs`,
`WafConfig::validate_shape` checks body configuration through
`prefix_size_bytes`, while `WafJsonConfig::inspection_limits` copies the
explicit values into the engine limits. Raising an individual ceiling permits
more work or retention under that budget; other independent limits still apply.

Determine whether explicit profiles need hard field ceilings, which ceilings
are appropriate, and how shared validation should enforce them for Controller
and Gateway. The documented complete override and restrictive zero semantics
are relevant to that decision. This configuration question is independent of
the accepted per-unit accounting for ordinary rule work.
